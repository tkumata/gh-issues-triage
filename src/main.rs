use std::{
    env,
    fmt::{self, Display, Formatter},
    io::{self, Read, Write},
    process,
    time::Duration,
};

use reqwest::blocking::Client;

mod auth;
mod branch;
mod cli;
mod credentials;
mod github;
mod jev;
mod model;
mod presentation;

use auth::AuthError;
use cli::{Command, USAGE, parse_args};
use credentials::CredentialError;
use github::{GithubError, fetch_issues};
use jev::{JevError, triage_issues, typesafe_api_key};
use model::RepositoryRef;
use presentation::{
    ButtonRegion, DisplayError, RenderedTable, render_table_with_buttons, terminal_height,
    terminal_width, wrap_line,
};

const USER_AGENT: &str = "gh-issues-triage/0.1.0";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum AppError {
    Auth(AuthError),
    Credential(CredentialError),
    Github(GithubError),
    Jev(JevError),
    Display(DisplayError),
    Interaction(String),
}

impl From<AuthError> for AppError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
impl From<GithubError> for AppError {
    fn from(error: GithubError) -> Self {
        Self::Github(error)
    }
}
impl From<CredentialError> for AppError {
    fn from(error: CredentialError) -> Self {
        Self::Credential(error)
    }
}
impl From<JevError> for AppError {
    fn from(error: JevError) -> Self {
        Self::Jev(error)
    }
}
impl From<DisplayError> for AppError {
    fn from(error: DisplayError) -> Self {
        Self::Display(error)
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(error) => Display::fmt(error, formatter),
            Self::Credential(error) => Display::fmt(error, formatter),
            Self::Github(error) => Display::fmt(error, formatter),
            Self::Jev(error) => Display::fmt(error, formatter),
            Self::Display(error) => Display::fmt(error, formatter),
            Self::Interaction(error) => formatter.write_str(error),
        }
    }
}

fn http_client() -> Result<Client, AuthError> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|_| AuthError::HttpRequest { timeout: false })
}

fn device_authorize(client: &Client) -> Result<credentials::Credentials, AppError> {
    credentials::ensure_available()?;
    let client_id = auth::client_id()?;
    let device = auth::request_device_code(client, &client_id)?;
    println!(
        "Open {} and enter code {}",
        device.verification_uri, device.user_code
    );
    let tokens = auth::poll_access_token(client, &client_id, &device)?;
    let credentials = credentials::Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    };
    Ok(credentials)
}

fn run_repository(repository: &RepositoryRef) -> Result<(), AppError> {
    let client = http_client()?;
    let stored = match credentials::load_credentials() {
        Ok(credentials) => Some(credentials),
        Err(CredentialError::NotLoggedIn) => None,
        Err(error) => return Err(AppError::Credential(error)),
    };
    let stored = if let Some(credentials) = stored {
        credentials
    } else {
        let credentials = device_authorize(&client)?;
        credentials::save_credentials(&credentials)?;
        credentials
    };
    let issues = match fetch_issues(&client, &stored.access_token, repository) {
        Ok(issues) => issues,
        Err(GithubError::HttpStatus { status: 401 }) => {
            let refreshed = if let Some(refresh_token) = stored.refresh_token.as_deref() {
                let client_id = auth::client_id()?;
                match auth::refresh_access_token(&client, &client_id, refresh_token) {
                    Ok(tokens) => credentials::Credentials {
                        access_token: tokens.access_token,
                        refresh_token: tokens.refresh_token,
                    },
                    Err(AuthError::Failure(auth::AuthFailure::BadRefreshToken)) => {
                        device_authorize(&client)?
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                device_authorize(&client)?
            };
            credentials::save_credentials(&refreshed)?;
            fetch_issues(&client, &refreshed.access_token, repository)?
        }
        Err(error) => return Err(error.into()),
    };
    let ranked = if issues.is_empty() {
        triage_issues(&client, "", issues)?
    } else {
        triage_issues(&client, &typesafe_api_key()?, issues)?
    };
    let width = terminal_width()?;
    let table = render_table_with_buttons(&ranked, width)?;
    if ranked.is_empty() {
        println!("{}", table.text);
    } else {
        run_branch_actions(repository, &ranked, &table, width).map_err(AppError::Interaction)?;
    }
    Ok(())
}

struct TerminalMode {
    original: libc::termios,
    restored: bool,
}

fn raw_terminal(mut settings: libc::termios) -> libc::termios {
    settings.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
    settings.c_cc[libc::VMIN] = 0;
    settings.c_cc[libc::VTIME] = 1;
    settings
}

impl TerminalMode {
    fn enter() -> Result<Self, String> {
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &raw mut original) } != 0 {
            return Err("interactive terminal is unavailable".to_owned());
        }
        let raw = raw_terminal(original);
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw const raw) } != 0 {
            return Err("cannot enter interactive terminal mode".to_owned());
        }
        Ok(Self {
            original,
            restored: false,
        })
    }

    fn restore(&mut self) -> Result<(), String> {
        let mut first_error = None;
        let mut stdout = io::stdout();
        if let Err(error) = stdout
            .write_all(TERMINAL_RESTORE.as_bytes())
            .and_then(|()| stdout.flush())
        {
            first_error = Some(format!("cannot restore terminal display: {error}"));
        }
        if unsafe {
            libc::tcsetattr(
                libc::STDIN_FILENO,
                libc::TCSANOW,
                std::ptr::addr_of!(self.original),
            )
        } != 0
        {
            first_error.get_or_insert_with(|| "cannot restore terminal input mode".to_owned());
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            self.restored = true;
            Ok(())
        }
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        if !self.restored {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(TERMINAL_RESTORE.as_bytes());
            let _ = stdout.flush();
            unsafe {
                libc::tcsetattr(
                    libc::STDIN_FILENO,
                    libc::TCSANOW,
                    std::ptr::addr_of!(self.original),
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionInput {
    Key(u8),
    Mouse { x: usize, y: usize },
    Up,
    Down,
    Continue,
    Quit,
}
const TERMINAL_RESTORE: &str = "\x1b[?1000l\x1b[?1006l\x1b[?1049l";
const TERMINAL_ENTER: &str = "\x1b[?1049h\x1b[?1000h\x1b[?1006h";

fn parse_mouse_event(sequence: &str) -> Option<(usize, usize)> {
    let body = sequence.strip_prefix('<')?.strip_suffix('M')?;
    let mut fields = body.split(';');
    if fields.next()?.parse::<u8>().ok()? != 0 {
        return None;
    }
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

fn action_for(
    input: ActionInput,
    buttons: &[ButtonRegion],
    scroll: usize,
    viewport_rows: usize,
) -> Option<usize> {
    match input {
        ActionInput::Key(key @ b'1'..=b'9') => Some(usize::from(key - b'1')),
        ActionInput::Key(b'0') => Some(9),
        ActionInput::Mouse { x, y } => buttons.iter().find_map(|button| {
            button
                .line
                .checked_sub(scroll)
                .filter(|row| *row < viewport_rows && y == row + 1)
                .filter(|_| (button.x_start..=button.x_end).contains(&x))
                .map(|_| button.issue_index)
        }),
        _ => None,
    }
}

fn read_byte(reader: &mut impl Read) -> io::Result<Option<u8>> {
    let mut byte = [0];
    Ok((reader.read(&mut byte)? != 0).then_some(byte[0]))
}

fn read_input(reader: &mut impl Read) -> io::Result<ActionInput> {
    let Some(byte) = read_byte(reader)? else {
        return Ok(ActionInput::Continue);
    };
    if byte == b'q' || byte == 3 {
        return Ok(ActionInput::Quit);
    }
    if byte != 0x1b {
        return Ok(ActionInput::Key(byte));
    }
    if read_byte(reader)? != Some(b'[') {
        return Ok(ActionInput::Continue);
    }
    let mut sequence = Vec::with_capacity(16);
    while sequence.len() < 32 {
        let Some(byte) = read_byte(reader)? else {
            return Ok(ActionInput::Continue);
        };
        sequence.push(byte);
        if (0x40..=0x7e).contains(&byte) {
            break;
        }
    }
    let Ok(sequence) = std::str::from_utf8(&sequence) else {
        return Ok(ActionInput::Continue);
    };
    if sequence == "A" {
        return Ok(ActionInput::Up);
    }
    if sequence == "B" {
        return Ok(ActionInput::Down);
    }
    Ok(parse_mouse_event(sequence)
        .map_or(ActionInput::Continue, |(x, y)| ActionInput::Mouse { x, y }))
}

fn scroll_by(input: ActionInput, scroll: usize, max_scroll: usize) -> Option<usize> {
    match input {
        ActionInput::Down | ActionInput::Key(b'j') => {
            Some(scroll.saturating_add(1).min(max_scroll))
        }
        ActionInput::Up | ActionInput::Key(b'k') => Some(scroll.saturating_sub(1)),
        _ => None,
    }
}

fn visible_lines(lines: &[String], scroll: usize, rows: usize) -> Vec<&str> {
    lines
        .iter()
        .skip(scroll)
        .take(rows)
        .map(String::as_str)
        .collect()
}

fn wrap_message(message: &str, width: usize) -> Vec<String> {
    message
        .lines()
        .flat_map(|line| {
            let safe_line = line
                .chars()
                .map(|character| {
                    if character.is_control() {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>();
            wrap_line(&safe_line, width)
        })
        .collect()
}

fn draw_view(
    stdout: &mut impl Write,
    lines: &[String],
    scroll: usize,
    viewport_rows: usize,
    help: &[String],
) -> Result<(), String> {
    write!(stdout, "\x1b[2J\x1b[H").map_err(|error| error.to_string())?;
    let visible = visible_lines(lines, scroll, viewport_rows);
    for row in 0..viewport_rows {
        let line = visible.get(row).copied().unwrap_or("");
        write!(stdout, "\x1b[{};1H\x1b[2K{line}", row + 1).map_err(|error| error.to_string())?;
    }
    for (index, line) in help.iter().enumerate() {
        let row = viewport_rows + index + 1;
        write!(stdout, "\x1b[{row};1H\x1b[2K{line}").map_err(|error| error.to_string())?;
    }
    stdout.flush().map_err(|error| error.to_string())
}

fn result_dismisses(input: ActionInput) -> bool {
    matches!(input, ActionInput::Key(_) | ActionInput::Quit)
}

fn show_result(
    message: &str,
    width: usize,
    height: usize,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let lines = wrap_message(message, width);
    let help = wrap_line("j/k or arrows: scroll; any key: return", width);
    let viewport_rows = height.saturating_sub(help.len());
    if viewport_rows == 0 {
        return Err("terminal is too short to show the result".to_owned());
    }
    let mut scroll = 0;
    let max_scroll = lines.len().saturating_sub(viewport_rows);
    loop {
        draw_view(stdout, &lines, scroll, viewport_rows, &help)?;
        let input = read_input(stdin).map_err(|error| error.to_string())?;
        if let Some(next) = scroll_by(input, scroll, max_scroll) {
            scroll = next;
        } else if result_dismisses(input) {
            break;
        }
    }
    Ok(())
}

fn run_branch_actions(
    repository: &RepositoryRef,
    issues: &[model::RankedIssue],
    table: &RenderedTable,
    width: usize,
) -> Result<(), String> {
    let height = terminal_height().map_err(|error| error.to_string())?;
    let help = wrap_line(
        "Click a button; 1-9/0: create; j/k or arrows: scroll; q: quit",
        width,
    );
    let viewport_rows = height.saturating_sub(help.len());
    if viewport_rows == 0 {
        return Err("terminal is too short to show issue actions".to_owned());
    }
    let mut terminal = TerminalMode::enter()?;
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    write!(stdout, "{TERMINAL_ENTER}").map_err(|error| error.to_string())?;
    let lines = table.text.lines().map(str::to_owned).collect::<Vec<_>>();
    let max_scroll = lines.len().saturating_sub(viewport_rows);
    let mut scroll = 0;
    loop {
        draw_view(&mut stdout, &lines, scroll, viewport_rows, &help)?;
        let input = read_input(&mut stdin).map_err(|error| error.to_string())?;
        if input == ActionInput::Quit {
            break;
        }
        if let Some(next) = scroll_by(input, scroll, max_scroll) {
            scroll = next;
            continue;
        }
        if let Some(issue) = action_for(input, &table.buttons, scroll, viewport_rows)
            .and_then(|index| issues.get(index))
        {
            let message = branch::create_branch(repository, issue).map_or_else(
                |error| format!("Branch creation failed: {error}"),
                |name| format!("Created branch {name}"),
            );
            show_result(&message, width, height, &mut stdin, &mut stdout)?;
        }
    }
    terminal.restore()?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(reason) => {
            eprintln!("error: {reason}\n{USAGE}");
            process::exit(2);
        }
    };
    let result = match command {
        Command::Repository(repository) => run_repository(&repository),
        Command::SetRoot(root) => branch::set_root(&root).map_err(AppError::Interaction),
    };
    if let Err(reason) = result {
        eprintln!("error: {reason}");
        process::exit(1);
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn keyboard_mouse_bounds_escape_sequences_and_restore_controls() {
        let buttons = [
            ButtonRegion {
                issue_index: 0,
                x_start: 19,
                x_end: 21,
                line: 4,
            },
            ButtonRegion {
                issue_index: 1,
                x_start: 19,
                x_end: 21,
                line: 8,
            },
        ];
        assert_eq!(action_for(ActionInput::Key(b'1'), &buttons, 0, 24), Some(0));
        assert!(matches!(
            read_input(&mut Cursor::new(b"1")),
            Ok(ActionInput::Key(b'1'))
        ));
        assert_eq!(
            action_for(ActionInput::Mouse { x: 20, y: 5 }, &buttons, 0, 24),
            Some(0)
        );
        assert_eq!(
            action_for(ActionInput::Mouse { x: 22, y: 5 }, &buttons, 0, 24),
            None
        );
        assert_eq!(
            action_for(ActionInput::Mouse { x: 20, y: 4 }, &buttons, 0, 24),
            None
        );
        assert_eq!(
            action_for(ActionInput::Mouse { x: 20, y: 3 }, &buttons, 2, 4),
            Some(0)
        );
        assert_eq!(
            action_for(ActionInput::Mouse { x: 20, y: 3 }, &buttons, 6, 4),
            Some(1)
        );
        assert_eq!(
            action_for(ActionInput::Mouse { x: 20, y: 5 }, &buttons, 0, 4),
            None
        );
        assert_eq!(scroll_by(ActionInput::Key(b'j'), 0, 9), Some(1));
        assert_eq!(scroll_by(ActionInput::Down, 8, 9), Some(9));
        assert_eq!(scroll_by(ActionInput::Key(b'k'), 0, 9), Some(0));
        assert_eq!(parse_mouse_event("<0;20;5M"), Some((20, 5)));
        assert_eq!(parse_mouse_event("<32;20;5M"), None);
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[A")),
            Ok(ActionInput::Up)
        ));
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[B")),
            Ok(ActionInput::Down)
        ));
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[<0;20;5M")),
            Ok(ActionInput::Mouse { x: 20, y: 5 })
        ));
        let raw = raw_terminal(unsafe { std::mem::zeroed() });
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG), 0);
        assert_eq!(raw.c_cc[libc::VMIN], 0);
        assert_eq!(raw.c_cc[libc::VTIME], 1);
        assert_eq!(TERMINAL_ENTER, "\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        assert_eq!(TERMINAL_RESTORE, "\x1b[?1000l\x1b[?1006l\x1b[?1049l");
        let lines = vec!["one".to_owned(), "two".to_owned(), "three".to_owned()];
        assert_eq!(visible_lines(&lines, 1, 2), ["two", "three"]);
        let long_error = wrap_message("a very long reason", 5);
        assert_eq!(long_error.concat(), "a very long reason");
        assert!(long_error.len() > 1);
        let help = wrap_line(
            "Click a button; 1-9/0: create; j/k or arrows: scroll; q: quit",
            26,
        );
        assert!(help.len() > 1);
        assert_eq!(26_usize.saturating_sub(help.len()) + help.len(), 26);
    }

    #[test]
    fn drawing_positions_each_row_without_scrolling_and_result_ignores_no_input() {
        let mut output = Vec::new();
        assert!(
            draw_view(
                &mut output,
                &["table".to_owned()],
                0,
                2,
                &["help".to_owned()],
            )
            .is_ok()
        );
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("\x1b[1;1H\x1b[2Ktable"));
        assert!(output.contains("\x1b[2;1H\x1b[2K"));
        assert!(output.contains("\x1b[3;1H\x1b[2Khelp"));
        assert!(!output.ends_with('\n'));
        let border = "┌────┐";
        let mut border_output = Vec::new();
        assert!(draw_view(&mut border_output, &[border.to_owned()], 0, 1, &[]).is_ok());
        let border_output = String::from_utf8_lossy(&border_output);
        assert!(border_output.contains("\x1b[1;1H\x1b[2K┌────┐"));
        assert!(!border_output.contains("\x1b[K"));

        let no_input = read_input(&mut Cursor::new(b"")).unwrap_or(ActionInput::Quit);
        assert_eq!(no_input, ActionInput::Continue);
        assert!(!result_dismisses(no_input));
        assert!(result_dismisses(ActionInput::Key(b'x')));
    }
}
