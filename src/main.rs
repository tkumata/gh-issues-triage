use std::{
    collections::BTreeSet,
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
use model::{IssueSort, RankedIssue, RepositoryRef, SortColumn, next_ready_issue, search_issues};
use presentation::{
    ButtonRegion, DisplayError, RenderedTable, SEARCH_BUTTON, SEARCH_ROWS, render_next_json,
    render_search_input, render_search_results, render_table_with_buttons, terminal_height,
    terminal_width, wrap_line,
};

const USER_AGENT: &str = "gh-issues-triage/0.1.0";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum AppError {
    Auth(AuthError),
    TokenRefresh(AuthError),
    DeviceFlow(AuthError),
    Credential(CredentialError),
    Github(GithubError),
    Jev(JevError),
    Display(DisplayError),
    Json(serde_json::Error),
    Output(io::Error),
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
            Self::TokenRefresh(error) => write!(formatter, "GitHub token refresh failed: {error}"),
            Self::DeviceFlow(error) => {
                write!(formatter, "GitHub device authorization failed: {error}")
            }
            Self::Credential(error) => Display::fmt(error, formatter),
            Self::Github(error) => Display::fmt(error, formatter),
            Self::Jev(error) => Display::fmt(error, formatter),
            Self::Display(error) => Display::fmt(error, formatter),
            Self::Json(error) => write!(formatter, "cannot serialize JSON: {error}"),
            Self::Output(error) => write!(formatter, "cannot write output: {error}"),
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

fn device_authorize(
    client: &Client,
    notice: &mut dyn Write,
) -> Result<credentials::Credentials, AppError> {
    credentials::ensure_available()?;
    let client_id = auth::client_id().map_err(AppError::DeviceFlow)?;
    let device = auth::request_device_code(client, &client_id).map_err(AppError::DeviceFlow)?;
    writeln!(
        notice,
        "Open {} and enter code {}",
        device.verification_uri, device.user_code
    )
    .map_err(AppError::Output)?;
    notice.flush().map_err(AppError::Output)?;
    let tokens =
        auth::poll_access_token(client, &client_id, &device).map_err(AppError::DeviceFlow)?;
    let credentials = credentials::Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    };
    Ok(credentials)
}

fn fetch_authenticated_issues(
    stored: &credentials::Credentials,
    notice: &mut dyn Write,
    mut fetch: impl FnMut(&str) -> Result<Vec<model::Issue>, GithubError>,
    refresh: impl FnOnce(&str) -> Result<auth::TokenSet, AuthError>,
    authorize: impl FnOnce(&mut dyn Write) -> Result<credentials::Credentials, AppError>,
    save: impl FnOnce(&credentials::Credentials) -> Result<(), CredentialError>,
) -> Result<Vec<model::Issue>, AppError> {
    match fetch(&stored.access_token) {
        Ok(issues) => return Ok(issues),
        Err(GithubError::HttpStatus { status: 401 }) => {}
        Err(error) => return Err(error.into()),
    }
    let renewed = if let Some(refresh_token) = stored.refresh_token.as_deref() {
        match refresh(refresh_token) {
            Ok(tokens) => credentials::Credentials {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
            },
            Err(AuthError::Failure(auth::AuthFailure::BadRefreshToken)) => authorize(notice)?,
            Err(AuthError::Failure(auth::AuthFailure::IncorrectClientCredentials)) => {
                writeln!(
                    notice,
                    "GitHub rejected client credentials during token refresh; device authorization is required."
                )
                .map_err(AppError::Output)?;
                notice.flush().map_err(AppError::Output)?;
                authorize(notice)?
            }
            Err(error) => return Err(AppError::TokenRefresh(error)),
        }
    } else {
        authorize(notice)?
    };
    save(&renewed)?;
    Ok(fetch(&renewed.access_token)?)
}

fn load_ranked_issues(
    repository: &RepositoryRef,
    notice: &mut impl Write,
    include_details: bool,
) -> Result<Vec<RankedIssue>, AppError> {
    let client = http_client()?;
    let stored = match credentials::load_credentials() {
        Ok(credentials) => Some(credentials),
        Err(CredentialError::NotLoggedIn) => None,
        Err(error) => return Err(AppError::Credential(error)),
    };
    let stored = if let Some(credentials) = stored {
        credentials
    } else {
        let credentials = device_authorize(&client, notice)?;
        credentials::save_credentials(&credentials)?;
        credentials
    };
    let issues = fetch_authenticated_issues(
        &stored,
        notice,
        |token| fetch_issues(&client, token, repository, include_details),
        |refresh_token| {
            let client_id = auth::client_id()?;
            auth::refresh_access_token(&client, &client_id, refresh_token)
        },
        |notice| device_authorize(&client, notice),
        credentials::save_credentials,
    )?;
    if issues.is_empty() {
        Ok(triage_issues(&client, "", issues)?)
    } else {
        Ok(triage_issues(&client, &typesafe_api_key()?, issues)?)
    }
}

fn run_repository(repository: &RepositoryRef) -> Result<(), AppError> {
    let ranked = load_ranked_issues(repository, &mut io::stdout(), true)?;
    let width = terminal_width()?;
    let branches = if ranked.is_empty() {
        Ok(BTreeSet::new())
    } else {
        branch::local_issue_branches(repository)
    };
    let table = render_table_with_buttons(&ranked, width, &branches, IssueSort::default())?;
    if ranked.is_empty() {
        println!("{}", table.text);
    } else {
        run_branch_actions(repository, &ranked, table, branches, width)
            .map_err(AppError::Interaction)?;
    }
    Ok(())
}

fn write_next_json(
    repository: &RepositoryRef,
    ranked: &[RankedIssue],
    output: &mut impl Write,
) -> Result<(), AppError> {
    let json = render_next_json(repository, next_ready_issue(ranked)).map_err(AppError::Json)?;
    writeln!(output, "{json}").map_err(AppError::Output)?;
    output.flush().map_err(AppError::Output)
}

fn run_next(repository: &RepositoryRef) -> Result<(), AppError> {
    let ranked = load_ranked_issues(repository, &mut io::stderr(), false)?;
    write_next_json(repository, &ranked, &mut io::stdout().lock())
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
    PasteByte(u8),
    Mouse { x: usize, y: usize },
    Hover { x: usize, y: usize },
    Wheel { x: usize, y: usize, down: bool },
    Up,
    Down,
    PageUp,
    PageDown,
    Continue,
    Search,
    Quit,
}

#[derive(Default)]
struct SearchState {
    draft: String,
    query: String,
    pending_utf8: Vec<u8>,
    focused: bool,
}

#[derive(Default)]
struct ListState {
    sort: IssueSort,
    selected: Option<usize>,
    scroll: usize,
}

impl ListState {
    fn sort_by(&mut self, column: SortColumn, visible: &mut [RankedIssue]) {
        self.sort.toggle(column);
        self.sort.apply(visible);
        self.selected = None;
    }
}

impl SearchState {
    fn submit(&mut self) -> io::Result<ActionInput> {
        if !self.pending_utf8.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete UTF-8 input",
            ));
        }
        self.query.clone_from(&self.draft);
        self.focused = false;
        Ok(ActionInput::Search)
    }

    fn push_byte(&mut self, byte: u8) -> io::Result<()> {
        self.pending_utf8.push(byte);
        match std::str::from_utf8(&self.pending_utf8) {
            Ok(text) => {
                if text.chars().any(char::is_control) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "control character in search input",
                    ));
                }
                self.draft.push_str(text);
                self.pending_utf8.clear();
            }
            Err(error) if error.error_len().is_none() => {}
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
        Ok(())
    }

    fn route(&mut self, input: ActionInput) -> io::Result<ActionInput> {
        let routed = match input {
            ActionInput::PasteByte(byte) => {
                if self.focused {
                    match byte {
                        b'\t' | b'\r' | b'\n' => self.push_byte(b' ')?,
                        32..=126 | 128..=255 => self.push_byte(byte)?,
                        _ => {}
                    }
                }
                ActionInput::Continue
            }
            ActionInput::Key(b'\t') => {
                self.focused = !self.focused;
                ActionInput::Continue
            }
            ActionInput::Mouse { y: 1, .. } => {
                self.focused = true;
                ActionInput::Continue
            }
            ActionInput::Mouse { x, y: 2 } if x <= SEARCH_BUTTON.len() => return self.submit(),
            ActionInput::Mouse {
                y: 2..=SEARCH_ROWS, ..
            } => ActionInput::Continue,
            ActionInput::Mouse { x, y } => {
                self.focused = false;
                ActionInput::Mouse {
                    x,
                    y: y.saturating_sub(SEARCH_ROWS),
                }
            }
            ActionInput::Hover { .. }
            | ActionInput::Up
            | ActionInput::Down
            | ActionInput::PageUp
            | ActionInput::PageDown
                if self.focused =>
            {
                ActionInput::Continue
            }
            ActionInput::Wheel { x, y, down } => ActionInput::Wheel {
                x,
                y: y.saturating_sub(SEARCH_ROWS),
                down,
            },
            ActionInput::Hover { x, y } => ActionInput::Hover {
                x,
                y: y.saturating_sub(SEARCH_ROWS),
            },
            ActionInput::Key(b'\r' | b'\n') if self.focused => return self.submit(),
            ActionInput::Key(8 | 127) if self.focused => {
                if self.pending_utf8.is_empty() {
                    self.draft.pop();
                } else {
                    self.pending_utf8.clear();
                }
                ActionInput::Continue
            }
            ActionInput::Key(byte) if self.focused => {
                if byte >= b' ' {
                    self.push_byte(byte)?;
                }
                ActionInput::Continue
            }
            ActionInput::Key(b'q') => ActionInput::Quit,
            other => other,
        };
        Ok(routed)
    }
}

const TERMINAL_RESTORE: &str = "\x1b[?2004l\x1b[?1003l\x1b[?1000l\x1b[?1006l\x1b[?1049l";
const TERMINAL_ENTER: &str = "\x1b[?1049h\x1b[?1000h\x1b[?1003h\x1b[?1006h\x1b[?2004h";

fn parse_mouse_event(sequence: &str) -> Option<ActionInput> {
    let body = sequence.strip_prefix('<')?.strip_suffix('M')?;
    let mut fields = body.split(';');
    let button = fields.next()?.parse::<u8>().ok()?;
    let x = fields.next()?.parse().ok()?;
    let y = fields.next()?.parse().ok()?;
    if x == 0 || y == 0 || fields.next().is_some() {
        return None;
    }
    match button {
        0 => Some(ActionInput::Mouse { x, y }),
        32..=35 => Some(ActionInput::Hover { x, y }),
        64 | 65 => Some(ActionInput::Wheel {
            x,
            y,
            down: button == 65,
        }),
        _ => None,
    }
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

fn sort_column_for(
    input: ActionInput,
    table: &RenderedTable,
    scroll: usize,
    viewport_rows: usize,
) -> Option<SortColumn> {
    let ActionInput::Mouse { x, y } = input else {
        return None;
    };
    table.sort_headers.iter().find_map(|header| {
        header
            .line
            .checked_sub(scroll)
            .filter(|row| *row < viewport_rows && y == row + 1)
            .filter(|_| (header.x_start..=header.x_end).contains(&x))
            .map(|_| header.column)
    })
}

fn read_byte(reader: &mut impl Read) -> io::Result<Option<u8>> {
    let mut byte = [0];
    Ok((reader.read(&mut byte)? != 0).then_some(byte[0]))
}

#[derive(Default)]
struct InputState {
    escape: Vec<u8>,
    pasting: bool,
}

impl InputState {
    fn read_input(&mut self, reader: &mut impl Read) -> io::Result<ActionInput> {
        loop {
            let Some(byte) = read_byte(reader)? else {
                return Ok(ActionInput::Continue);
            };
            if byte == 3 && !self.pasting {
                self.escape.clear();
                return Ok(ActionInput::Quit);
            }
            if self.escape.is_empty() && byte != 0x1b {
                return Ok(if self.pasting {
                    ActionInput::PasteByte(byte)
                } else {
                    ActionInput::Key(byte)
                });
            }
            self.escape.push(byte);
            if self.escape.len() == 2 && byte != b'[' {
                self.escape.clear();
                return Ok(ActionInput::Continue);
            }
            if self.escape.len() > 2 && (self.escape.len() >= 32 || (0x40..=0x7e).contains(&byte)) {
                return Ok(self.sequence_action());
            }
        }
    }

    fn sequence_action(&mut self) -> ActionInput {
        let sequence = std::mem::take(&mut self.escape);
        match sequence.as_slice() {
            b"\x1b[200~" => {
                self.pasting = true;
                ActionInput::Continue
            }
            b"\x1b[201~" => {
                self.pasting = false;
                ActionInput::Continue
            }
            _ if self.pasting => ActionInput::Continue,
            b"\x1b[A" => ActionInput::Up,
            b"\x1b[B" => ActionInput::Down,
            b"\x1b[5~" => ActionInput::PageUp,
            b"\x1b[6~" => ActionInput::PageDown,
            _ => sequence
                .strip_prefix(b"\x1b[")
                .and_then(|body| std::str::from_utf8(body).ok())
                .and_then(parse_mouse_event)
                .unwrap_or(ActionInput::Continue),
        }
    }
}

fn hovered_issue(
    table: &RenderedTable,
    x: usize,
    y: usize,
    scroll: usize,
    viewport_rows: usize,
    width: usize,
) -> Option<usize> {
    if x == 0 || x > width || y == 0 || y > viewport_rows {
        return None;
    }
    let line = scroll.checked_add(y - 1)?;
    table
        .issues
        .iter()
        .position(|issue| issue.lines.contains(&line))
}

fn first_visible_issue(
    table: &RenderedTable,
    scroll: usize,
    viewport_rows: usize,
) -> Option<usize> {
    table.issues.iter().position(|issue| {
        issue.lines.end > scroll && issue.lines.start < scroll.saturating_add(viewport_rows)
    })
}

fn navigate_issue(
    input: ActionInput,
    table: &RenderedTable,
    selected: Option<usize>,
    scroll: usize,
    viewport_rows: usize,
) -> Option<(usize, usize)> {
    let last = table.issues.len().checked_sub(1)?;
    let next = match (input, selected) {
        (ActionInput::Down | ActionInput::Key(b'j'), Some(index)) => {
            index.saturating_add(1).min(last)
        }
        (ActionInput::Up | ActionInput::Key(b'k'), Some(index)) => index.saturating_sub(1),
        (ActionInput::Up | ActionInput::Down | ActionInput::Key(b'j' | b'k'), None) => {
            first_visible_issue(table, scroll, viewport_rows).unwrap_or(0)
        }
        _ => return None,
    };
    let issue = table.issues.get(next)?;
    let next_scroll = if issue.lines.start < scroll || issue.lines.len() > viewport_rows {
        issue.lines.start
    } else {
        scroll.max(issue.lines.end.saturating_sub(viewport_rows))
    };
    let max_scroll = table.text.lines().count().saturating_sub(viewport_rows);
    Some((next, next_scroll.min(max_scroll)))
}

fn scroll_by(
    input: ActionInput,
    scroll: usize,
    max_scroll: usize,
    viewport_rows: usize,
) -> Option<usize> {
    match input {
        ActionInput::Down | ActionInput::Key(b'j') => {
            Some(scroll.saturating_add(1).min(max_scroll))
        }
        ActionInput::Up | ActionInput::Key(b'k') => Some(scroll.saturating_sub(1)),
        ActionInput::PageDown => Some(scroll.saturating_add(viewport_rows).min(max_scroll)),
        ActionInput::PageUp => Some(scroll.saturating_sub(viewport_rows)),
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
        .flat_map(|line| wrap_line(line, width))
        .collect()
}

fn draw_view(
    stdout: &mut impl Write,
    lines: &[String],
    scroll: usize,
    viewport_rows: usize,
    help: &[String],
    header: &[String],
) -> Result<(), String> {
    write!(stdout, "\x1b[2J\x1b[H").map_err(|error| error.to_string())?;
    for (row, line) in header.iter().enumerate() {
        write!(stdout, "\x1b[{};1H\x1b[2K{line}", row + 1).map_err(|error| error.to_string())?;
    }
    let visible = visible_lines(lines, scroll, viewport_rows);
    for row in 0..viewport_rows {
        let line = visible.get(row).copied().unwrap_or("");
        write!(stdout, "\x1b[{};1H\x1b[2K{line}", header.len() + row + 1)
            .map_err(|error| error.to_string())?;
    }
    for (index, line) in help.iter().enumerate() {
        let row = header.len() + viewport_rows + index + 1;
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
    input_state: &mut InputState,
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
        draw_view(stdout, &lines, scroll, viewport_rows, &help, &[])?;
        let input = input_state
            .read_input(stdin)
            .map_err(|error| error.to_string())?;
        if let Some(next) = scroll_by(input, scroll, max_scroll, viewport_rows) {
            scroll = next;
        } else if result_dismisses(input) {
            break;
        }
    }
    Ok(())
}

fn draw_list_view(
    stdout: &mut impl Write,
    table: &mut RenderedTable,
    list: &mut ListState,
    search: &SearchState,
    height: usize,
    width: usize,
    action_help: &[String],
) -> Result<usize, String> {
    let mut help = action_help.to_vec();
    help.extend(table.branch_help.iter().cloned());
    let viewport_rows = height.saturating_sub(help.len() + SEARCH_ROWS);
    if viewport_rows == 0 {
        return Err("terminal is too short to show issue actions".to_owned());
    }
    let importance_rows = wrap_line("重要度: 0（低）〜4（高）", width).len();
    table
        .limit_rows(viewport_rows.saturating_sub(importance_rows))
        .map_err(|error| error.to_string())?;
    let max_scroll = table.text.lines().count().saturating_sub(viewport_rows);
    list.scroll = list.scroll.min(max_scroll);
    let (header, cursor) = render_search_input(&search.draft, search.focused, width);
    draw_view(
        stdout,
        &table.lines(list.selected),
        list.scroll,
        viewport_rows,
        &help,
        &header,
    )?;
    if search.focused {
        write!(stdout, "\x1b[1;{cursor}H")
            .and_then(|()| stdout.flush())
            .map_err(|error| error.to_string())?;
    }
    Ok(viewport_rows)
}

fn run_branch_actions(
    repository: &RepositoryRef,
    issues: &[model::RankedIssue],
    mut table: RenderedTable,
    mut branches: Result<BTreeSet<u64>, String>,
    width: usize,
) -> Result<(), String> {
    let height = terminal_height().map_err(|error| error.to_string())?;
    let action_help = wrap_line(
        "Click headers: sort; Tab: search/list; Enter or Search: search; 1-9/0: create; j/k or arrows: select; PgUp/PgDn: scroll list; wheel: scroll cell; q: quit (list); Ctrl+C: quit",
        width,
    );
    let mut terminal = TerminalMode::enter()?;
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    write!(stdout, "{TERMINAL_ENTER}").map_err(|error| error.to_string())?;
    let mut list = ListState::default();
    let mut search = SearchState::default();
    let mut input_state = InputState::default();
    let mut visible = issues.to_vec();
    loop {
        let viewport_rows = draw_list_view(
            &mut stdout,
            &mut table,
            &mut list,
            &search,
            height,
            width,
            &action_help,
        )?;
        let max_scroll = table.text.lines().count().saturating_sub(viewport_rows);
        let input = input_state
            .read_input(&mut stdin)
            .and_then(|input| search.route(input))
            .map_err(|error| error.to_string())?;
        if input == ActionInput::Quit {
            break;
        }
        if input == ActionInput::Search {
            visible = search_issues(issues, &search.query);
            list.sort.apply(&mut visible);
            table = render_search_results(&visible, width, &branches, list.sort)
                .map_err(|error| error.to_string())?;
            list.selected = None;
            list.scroll = 0;
            continue;
        }
        if let ActionInput::Wheel { x, y, down } = input {
            if (1..=viewport_rows).contains(&y) {
                table.scroll_cell(x, list.scroll + y - 1, down);
            }
            continue;
        }
        if let Some(column) = sort_column_for(input, &table, list.scroll, viewport_rows) {
            list.sort_by(column, &mut visible);
            table = render_search_results(&visible, width, &branches, list.sort)
                .map_err(|error| error.to_string())?;
            continue;
        }
        if let Some((index, next)) =
            navigate_issue(input, &table, list.selected, list.scroll, viewport_rows)
        {
            list.selected = Some(index);
            list.scroll = next;
            continue;
        }
        if let Some(next) = scroll_by(input, list.scroll, max_scroll, viewport_rows) {
            list.scroll = next;
            continue;
        }
        if let ActionInput::Hover { x, y } = input {
            list.selected = hovered_issue(&table, x, y, list.scroll, viewport_rows, width);
            continue;
        }
        if let Some(issue) = action_for(input, &table.buttons, list.scroll, viewport_rows)
            .and_then(|index| visible.get(index))
        {
            let result = branch::create_branch(repository, issue);
            let succeeded = result.is_ok();
            let message = result.map_or_else(
                |error| format!("Branch creation failed: {error}"),
                |name| format!("Created branch {name}"),
            );
            show_result(
                &message,
                width,
                height,
                &mut stdin,
                &mut stdout,
                &mut input_state,
            )?;
            if succeeded {
                branches = branch::local_issue_branches(repository);
                let mut updated = render_search_results(&visible, width, &branches, list.sort)
                    .map_err(|error| error.to_string())?;
                updated.preserve_cell_offsets(&table);
                table = updated;
            }
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
        Command::Help => {
            println!("{USAGE}");
            return;
        }
        Command::Repository(repository) => run_repository(&repository),
        Command::Next(repository) => run_next(&repository),
        Command::SetRoot(root) => branch::set_root(&root).map_err(AppError::Interaction),
    };
    if let Err(reason) = result {
        eprintln!("error: {reason}");
        process::exit(1);
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use std::cell::RefCell;

    struct Scenario {
        stored: credentials::Credentials,
        fetches: Vec<Result<Vec<model::Issue>, GithubError>>,
        refresh: Result<auth::TokenSet, AuthError>,
        authorize: Result<credentials::Credentials, AppError>,
        save: Result<(), CredentialError>,
    }

    impl Default for Scenario {
        fn default() -> Self {
            Self {
                stored: credentials::Credentials {
                    access_token: "saved-access".into(),
                    refresh_token: Some("saved-refresh".into()),
                },
                fetches: vec![Err(GithubError::HttpStatus { status: 401 }), Ok(vec![])],
                refresh: Ok(auth::TokenSet {
                    access_token: "refreshed-access".into(),
                    refresh_token: Some("refreshed-refresh".into()),
                }),
                authorize: Ok(credentials::Credentials {
                    access_token: "authorized-access".into(),
                    refresh_token: Some("authorized-refresh".into()),
                }),
                save: Ok(()),
            }
        }
    }

    struct Outcome {
        result: Result<Vec<model::Issue>, AppError>,
        events: Vec<String>,
        notice: Vec<u8>,
    }

    impl Scenario {
        fn run(self) -> Outcome {
            let events = RefCell::new(Vec::new());
            let mut fetches = self.fetches.into_iter();
            let mut notice = Vec::new();
            let result = fetch_authenticated_issues(
                &self.stored,
                &mut notice,
                |token| {
                    events.borrow_mut().push(format!("fetch:{token}"));
                    fetches.next().unwrap_or(Err(GithubError::InvalidResponse))
                },
                |token| {
                    events.borrow_mut().push(format!("refresh:{token}"));
                    self.refresh
                },
                |_| {
                    events.borrow_mut().push("authorize".into());
                    self.authorize
                },
                |credentials| {
                    events.borrow_mut().push(format!(
                        "save:{}:{}",
                        credentials.access_token,
                        credentials.refresh_token.as_deref().unwrap_or("none")
                    ));
                    self.save
                },
            );
            Outcome {
                result,
                events: events.into_inner(),
                notice,
            }
        }
    }

    #[test]
    fn valid_saved_token_skips_refresh_authorization_and_save() {
        let outcome = Scenario {
            fetches: vec![Ok(vec![model::Issue {
                details: Some(crate::model::test_issue_details()),
                number: 31,
                title: "saved token works".into(),
                body: String::new(),
            }])],
            ..Scenario::default()
        }
        .run();
        assert!(outcome.result.is_ok_and(|issues| {
            issues.len() == 1 && issues.first().is_some_and(|issue| issue.number == 31)
        }));
        assert_eq!(outcome.events, ["fetch:saved-access"]);
        assert_eq!(outcome.notice, [] as [u8; 0]);
    }

    #[test]
    fn successful_refresh_saves_both_rotated_tokens_before_retry() {
        let outcome = Scenario::default().run();
        assert!(outcome.result.is_ok());
        assert_eq!(
            outcome.events,
            [
                "fetch:saved-access",
                "refresh:saved-refresh",
                "save:refreshed-access:refreshed-refresh",
                "fetch:refreshed-access",
            ]
        );
        assert_eq!(outcome.notice, [] as [u8; 0]);
    }

    #[test]
    fn rejected_refresh_reauthorizes_once_then_saves_and_retries() {
        for failure in [
            auth::AuthFailure::BadRefreshToken,
            auth::AuthFailure::IncorrectClientCredentials,
        ] {
            let needs_notice = failure == auth::AuthFailure::IncorrectClientCredentials;
            let outcome = Scenario {
                refresh: Err(AuthError::Failure(failure)),
                ..Scenario::default()
            }
            .run();
            assert!(outcome.result.is_ok());
            assert_eq!(
                outcome.events,
                [
                    "fetch:saved-access",
                    "refresh:saved-refresh",
                    "authorize",
                    "save:authorized-access:authorized-refresh",
                    "fetch:authorized-access",
                ]
            );
            if needs_notice {
                assert_eq!(
                    String::from_utf8(outcome.notice).ok().as_deref(),
                    Some(
                        "GitHub rejected client credentials during token refresh; device authorization is required.\n"
                    )
                );
            } else {
                assert_eq!(outcome.notice, [] as [u8; 0]);
            }
        }
    }

    #[test]
    fn absent_refresh_token_uses_device_flow_without_refresh() {
        let mut scenario = Scenario::default();
        scenario.stored.refresh_token = None;
        let outcome = scenario.run();
        assert!(outcome.result.is_ok());
        assert_eq!(
            outcome.events,
            [
                "fetch:saved-access",
                "authorize",
                "save:authorized-access:authorized-refresh",
                "fetch:authorized-access",
            ]
        );
    }

    #[test]
    fn issue_request_failures_do_not_refresh_or_reauthorize() {
        for error in [
            GithubError::HttpStatus { status: 403 },
            GithubError::HttpRequest { timeout: true },
        ] {
            let outcome = Scenario {
                fetches: vec![Err(error)],
                ..Scenario::default()
            }
            .run();
            assert!(matches!(outcome.result, Err(AppError::Github(_))));
            assert_eq!(outcome.events, ["fetch:saved-access"]);
        }
    }

    #[test]
    fn other_refresh_failures_stop_with_refresh_context() {
        for error in [
            AuthError::HttpRequest { timeout: true },
            AuthError::HttpStatus { status: 403 },
            AuthError::InvalidResponse,
            AuthError::MissingClientId,
        ] {
            let outcome = Scenario {
                refresh: Err(error),
                ..Scenario::default()
            }
            .run();
            assert!(matches!(outcome.result, Err(AppError::TokenRefresh(_))));
            assert_eq!(
                outcome.events,
                ["fetch:saved-access", "refresh:saved-refresh"]
            );
            assert_eq!(outcome.notice, [] as [u8; 0]);
        }
    }

    #[test]
    fn rejected_device_authorization_stops_without_save_or_retry() {
        let outcome = Scenario {
            refresh: Err(AuthError::Failure(
                auth::AuthFailure::IncorrectClientCredentials,
            )),
            authorize: Err(AppError::DeviceFlow(AuthError::Failure(
                auth::AuthFailure::IncorrectClientCredentials,
            ))),
            ..Scenario::default()
        }
        .run();
        let error = outcome.result.err();
        assert!(matches!(error, Some(AppError::DeviceFlow(_))));
        assert_eq!(
            error.map(|error| error.to_string()).as_deref(),
            Some("GitHub device authorization failed: client credentials were rejected")
        );
        assert_eq!(
            outcome.events,
            ["fetch:saved-access", "refresh:saved-refresh", "authorize"]
        );
    }

    #[test]
    fn save_failure_stops_before_retry() {
        let outcome = Scenario {
            refresh: Err(AuthError::Failure(
                auth::AuthFailure::IncorrectClientCredentials,
            )),
            save: Err(CredentialError::CredentialWriteFailed),
            ..Scenario::default()
        }
        .run();
        assert!(matches!(
            outcome.result,
            Err(AppError::Credential(CredentialError::CredentialWriteFailed))
        ));
        assert_eq!(
            outcome.events,
            [
                "fetch:saved-access",
                "refresh:saved-refresh",
                "authorize",
                "save:authorized-access:authorized-refresh",
            ]
        );
    }

    #[test]
    fn failed_reauthorization_notice_stops_before_device_flow() {
        let authorized = std::cell::Cell::new(false);
        let result = fetch_authenticated_issues(
            &Scenario::default().stored,
            &mut io::Cursor::new([]),
            |_| Err(GithubError::HttpStatus { status: 401 }),
            |_| {
                Err(AuthError::Failure(
                    auth::AuthFailure::IncorrectClientCredentials,
                ))
            },
            |_| {
                authorized.set(true);
                Err(AppError::DeviceFlow(AuthError::Expired))
            },
            |_| Ok(()),
        );
        assert!(matches!(result, Err(AppError::Output(_))));
        assert!(!authorized.get());
    }

    #[test]
    fn unauthorized_retry_stops_without_another_authorization() {
        let outcome = Scenario {
            fetches: vec![
                Err(GithubError::HttpStatus { status: 401 }),
                Err(GithubError::HttpStatus { status: 401 }),
            ],
            refresh: Err(AuthError::Failure(
                auth::AuthFailure::IncorrectClientCredentials,
            )),
            ..Scenario::default()
        }
        .run();
        assert!(matches!(
            outcome.result,
            Err(AppError::Github(GithubError::HttpStatus { status: 401 }))
        ));
        assert_eq!(
            outcome.events,
            [
                "fetch:saved-access",
                "refresh:saved-refresh",
                "authorize",
                "save:authorized-access:authorized-refresh",
                "fetch:authorized-access",
            ]
        );
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use std::io::Cursor;

    fn sorting_issues() -> [RankedIssue; 3] {
        [42, 7, 3].map(|number| RankedIssue {
            issue: model::Issue {
                number,
                title: format!("issue {number}"),
                body: String::new(),
                details: Some(model::test_issue_details()),
            },
            score: 3.0,
            prefix: "feat".into(),
            readiness: model::Readiness::Yes,
        })
    }

    #[test]
    fn header_click_boundaries_exclude_other_columns_and_preserve_the_search_draft() {
        let issues = sorting_issues();
        let table =
            render_table_with_buttons(&issues, 110, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        assert_eq!(
            table
                .sort_headers
                .iter()
                .map(|header| header.column)
                .collect::<Vec<_>>(),
            [
                SortColumn::Importance,
                SortColumn::Number,
                SortColumn::Author,
                SortColumn::CreatedAt
            ]
        );
        for header in &table.sort_headers {
            for x in [header.x_start, header.x_end] {
                for scroll in [0, 1] {
                    let mut search = SearchState {
                        focused: true,
                        draft: "unapplied".into(),
                        query: "issue".into(),
                        ..SearchState::default()
                    };
                    let input = search.route(ActionInput::Mouse {
                        x,
                        y: header.line - scroll + 1 + SEARCH_ROWS,
                    });
                    assert!(input.is_ok());
                    let Ok(input) = input else { return };
                    assert_eq!(
                        sort_column_for(input, &table, scroll, 100),
                        Some(header.column)
                    );
                    assert_eq!(search.query, "issue");
                    assert_eq!(search.draft, "unapplied");
                    assert!(!search.focused);
                    assert_eq!(action_for(input, &table.buttons, scroll, 100), None);
                }
            }
            for x in [header.x_start - 1, header.x_end + 1] {
                assert_eq!(
                    sort_column_for(
                        ActionInput::Mouse {
                            x,
                            y: header.line + 1
                        },
                        &table,
                        0,
                        100
                    ),
                    None
                );
            }
            for input in [
                ActionInput::Hover {
                    x: header.x_start,
                    y: header.line + 1,
                },
                ActionInput::Key(b'1'),
                ActionInput::Continue,
            ] {
                assert_eq!(sort_column_for(input, &table, 0, 100), None);
            }
            assert_eq!(
                sort_column_for(
                    ActionInput::Mouse {
                        x: header.x_start,
                        y: 1
                    },
                    &table,
                    header.line + 1,
                    100
                ),
                None
            );
            assert_eq!(
                sort_column_for(
                    ActionInput::Mouse {
                        x: header.x_start,
                        y: header.line + 1
                    },
                    &table,
                    0,
                    1
                ),
                None
            );
        }
    }

    #[test]
    fn sorting_clears_selection_preserves_scroll_and_survives_search_and_reset() {
        let issues = sorting_issues();
        let mut visible = issues.to_vec();
        let mut list = ListState {
            selected: Some(1),
            scroll: 1,
            ..ListState::default()
        };
        list.sort_by(SortColumn::Number, &mut visible);
        assert_eq!(list.selected, None);
        assert_eq!(list.scroll, 1);
        assert_eq!(
            visible
                .iter()
                .map(|issue| issue.issue.number)
                .collect::<Vec<_>>(),
            [3, 7, 42]
        );
        let mut search = SearchState {
            draft: "issue".into(),
            ..SearchState::default()
        };
        assert!(search.submit().is_ok());
        visible = search_issues(&issues, &search.query);
        list.sort.apply(&mut visible);
        let table = render_search_results(&visible, 110, &Ok(BTreeSet::from([42])), list.sort);
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        assert!(table.text.contains("番号↑"));
        assert_eq!(
            action_for(ActionInput::Key(b'1'), &table.buttons, 0, 100)
                .and_then(|index| visible.get(index))
                .map(|issue| issue.issue.number),
            Some(3)
        );
        for button in &table.buttons {
            assert_eq!(
                action_for(
                    ActionInput::Mouse {
                        x: button.x_start,
                        y: button.line + 1
                    },
                    &table.buttons,
                    0,
                    100
                )
                .and_then(|index| visible.get(index))
                .map(|issue| issue.issue.number),
                visible
                    .get(button.issue_index)
                    .map(|issue| issue.issue.number)
            );
        }
        search.draft.clear();
        assert!(search.submit().is_ok());
        visible = search_issues(&issues, &search.query);
        list.sort.apply(&mut visible);
        assert_eq!(
            visible
                .iter()
                .map(|issue| issue.issue.number)
                .collect::<Vec<_>>(),
            [3, 7, 42]
        );
    }

    fn read_input(reader: &mut impl Read) -> io::Result<ActionInput> {
        InputState::default().read_input(reader)
    }

    #[test]
    fn pasted_newlines_tabs_and_shortcuts_never_execute_actions() {
        let bytes = b"\x1b[200~title\n1\tqjk\x1b[201~";
        let mut reader = Cursor::new(bytes);
        let mut input_state = InputState::default();
        let mut search = SearchState {
            focused: true,
            ..SearchState::default()
        };
        for _ in 0..bytes.len() {
            let routed = input_state
                .read_input(&mut reader)
                .and_then(|input| search.route(input));
            assert_eq!(routed.ok(), Some(ActionInput::Continue));
        }
        assert_eq!(search.draft, "title 1 qjk");
        assert_eq!(search.query, "");
        assert!(search.focused);
        assert_eq!(
            input_state
                .read_input(&mut Cursor::new(b"\r"))
                .and_then(|input| search.route(input))
                .ok(),
            Some(ActionInput::Search)
        );
        assert_eq!(search.query, "title 1 qjk");
    }

    #[test]
    fn paste_state_survives_timeouts_and_never_dismisses_results_or_creates_branches() {
        let mut input_state = InputState::default();
        let mut search = SearchState::default();
        for chunk in [
            &b"\x1b["[..],
            &b"200"[..],
            &b"~"[..],
            &b"\x03q1\t\n"[..],
            &b"\x1b[<0;20;5M"[..],
            &b"\x1b[20"[..],
            &b"1~"[..],
        ] {
            let mut reader = Cursor::new(chunk);
            for _ in 0..=chunk.len() {
                let input = input_state.read_input(&mut reader);
                assert!(input.is_ok());
                let Ok(input) = input else { return };
                assert!(!result_dismisses(input));
                assert_eq!(search.route(input).ok(), Some(ActionInput::Continue));
            }
        }
        assert_eq!(search.draft, "");
        assert_eq!(
            input_state
                .read_input(&mut Cursor::new(b"q"))
                .and_then(|input| search.route(input))
                .ok(),
            Some(ActionInput::Quit)
        );
        assert_eq!(
            input_state.read_input(&mut Cursor::new(b"\x1b")).ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(
            input_state.read_input(&mut Cursor::new([3])).ok(),
            Some(ActionInput::Quit)
        );
    }

    #[test]
    fn search_input_accepts_split_utf8_and_consumes_shortcuts_until_submitted() {
        let mut search = SearchState::default();
        assert_eq!(
            search.route(ActionInput::Key(b'\t')).ok(),
            Some(ActionInput::Continue)
        );
        for byte in "qjk10日本🙂".bytes() {
            let input = read_input(&mut Cursor::new([byte]));
            assert_eq!(
                input.and_then(|input| search.route(input)).ok(),
                Some(ActionInput::Continue)
            );
            assert_eq!(
                search.route(ActionInput::Continue).ok(),
                Some(ActionInput::Continue)
            );
        }
        assert_eq!(search.draft, "qjk10日本🙂");
        assert_eq!(search.query, "");
        for input in [ActionInput::Up, ActionInput::Down, ActionInput::PageDown] {
            assert_eq!(search.route(input).ok(), Some(ActionInput::Continue));
        }
        assert_eq!(
            search.route(ActionInput::Key(127)).ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(search.draft, "qjk10日本");
        assert_eq!(
            search.route(ActionInput::Key(b'\r')).ok(),
            Some(ActionInput::Search)
        );
        assert_eq!(search.query, "qjk10日本");
        assert!(!search.focused);
        assert_eq!(
            search.route(ActionInput::Key(b'q')).ok(),
            Some(ActionInput::Quit)
        );
        assert!(search.route(ActionInput::Key(b'\t')).is_ok());
        assert_eq!(
            read_input(&mut Cursor::new([3]))
                .and_then(|input| search.route(input))
                .ok(),
            Some(ActionInput::Quit)
        );
    }

    #[test]
    fn search_button_applies_the_draft_and_invalid_utf8_is_an_error() {
        let mut search = SearchState {
            draft: "query".into(),
            ..SearchState::default()
        };
        assert_eq!(
            search.route(ActionInput::Mouse { x: 2, y: 1 }).ok(),
            Some(ActionInput::Continue)
        );
        assert!(search.focused);
        assert_eq!(
            search.route(ActionInput::Mouse { x: 2, y: 3 }).ok(),
            Some(ActionInput::Continue)
        );
        assert!(search.focused);
        assert_eq!(
            search.route(ActionInput::Hover { x: 20, y: 10 }).ok(),
            Some(ActionInput::Continue)
        );
        assert!(search.focused);
        assert_eq!(
            search
                .route(ActionInput::Mouse {
                    x: SEARCH_BUTTON.len() + 1,
                    y: 2
                })
                .ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(search.query, "");
        assert_eq!(
            search
                .route(ActionInput::Mouse {
                    x: SEARCH_BUTTON.len(),
                    y: 2
                })
                .ok(),
            Some(ActionInput::Search)
        );
        assert_eq!(search.query, "query");
        assert!(!search.focused);
        assert!(search.route(ActionInput::Key(b'\t')).is_ok());
        assert!(search.route(ActionInput::Key(b'x')).is_ok());
        assert_eq!(search.draft, "queryx");
        assert_eq!(search.query, "query");
        assert_eq!(
            search.route(ActionInput::Mouse { x: 11, y: 2 }).ok(),
            Some(ActionInput::Continue)
        );
        assert!(search.focused);
        assert_eq!(search.query, "query");
        assert_eq!(
            search.route(ActionInput::Mouse { x: 1, y: 2 }).ok(),
            Some(ActionInput::Search)
        );
        assert_eq!(search.query, "queryx");
        assert!(!search.focused);
        let mut invalid = SearchState {
            focused: true,
            ..SearchState::default()
        };
        assert!(
            matches!(invalid.route(ActionInput::Key(0xff)), Err(error) if error.kind() == io::ErrorKind::InvalidData)
        );
        let mut incomplete = SearchState {
            focused: true,
            ..SearchState::default()
        };
        assert!(incomplete.route(ActionInput::Key(0xe6)).is_ok());
        assert!(
            matches!(incomplete.submit(), Err(error) if error.kind() == io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn filtered_mouse_and_number_actions_target_the_displayed_issue_and_can_reset() {
        let all = [42, 7, 3].map(|number| RankedIssue {
            issue: model::Issue {
                details: Some(crate::model::test_issue_details()),
                number,
                title: format!("issue {number}"),
                body: String::new(),
            },
            score: 3.0,
            prefix: "fix".into(),
            readiness: model::Readiness::Yes,
        });
        let mut search = SearchState {
            draft: "7 issue".into(),
            ..SearchState::default()
        };
        assert!(search.submit().is_ok());
        let visible = search_issues(&all, &search.query);
        let table =
            render_search_results(&visible, 110, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        for button in &table.buttons {
            let scroll = 2;
            let click = search.route(ActionInput::Mouse {
                x: button.x_start,
                y: button.line - scroll + 1 + SEARCH_ROWS,
            });
            let index = click
                .ok()
                .and_then(|input| action_for(input, &table.buttons, scroll, 100));
            assert_eq!(
                index
                    .and_then(|index| visible.get(index))
                    .map(|issue| issue.issue.number),
                Some(7)
            );
        }
        for input in [ActionInput::Key(b'1'), ActionInput::Key(b'2')] {
            let selected =
                action_for(input, &table.buttons, 0, 100).and_then(|index| visible.get(index));
            assert_eq!(
                selected.map(|issue| issue.issue.number),
                (input == ActionInput::Key(b'1')).then_some(7)
            );
        }
        let hovered = search.route(ActionInput::Hover {
            x: 20,
            y: table
                .issues
                .first()
                .map_or(0, |issue| issue.lines.start + 1 + SEARCH_ROWS),
        });
        assert!(
            matches!(hovered, Ok(ActionInput::Hover { x, y }) if hovered_issue(&table, x, y, 0, 100, 110) == Some(0))
        );
        search.draft = "unapplied".into();
        let refreshed = render_search_results(
            &search_issues(&all, &search.query),
            110,
            &Ok(BTreeSet::from([7])),
            IssueSort::default(),
        );
        assert!(
            matches!(refreshed, Ok(table) if table.text.contains("🌿") && table.text.contains("issue 7") && !table.text.contains("issue 42"))
        );
        assert_eq!(search.query, "7 issue");
        assert!(search.submit().is_ok());
        let missing = search_issues(&all, &search.query);
        assert_eq!(missing, []);
        assert!(
            matches!(render_search_results(&missing, 110, &Ok(BTreeSet::new()), IssueSort::default()), Ok(table) if table.buttons.is_empty() && table.text.contains("No matching issues."))
        );
        search.draft.clear();
        assert!(search.submit().is_ok());
        assert_eq!(search_issues(&all, &search.query), all);
    }

    #[test]
    fn writes_next_json_to_a_non_terminal_and_propagates_write_errors() {
        let repository = RepositoryRef {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        let ranked = model::rank_issues(
            vec![model::Issue {
                details: Some(crate::model::test_issue_details()),
                number: 42,
                title: "title".into(),
                body: String::new(),
            }],
            vec![(3.6, "fix".into(), model::Readiness::Yes)],
        );
        let mut output = Vec::new();
        assert!(write_next_json(&repository, &ranked, &mut output).is_ok());
        assert!(
            output
                .strip_suffix(b"\n")
                .is_some_and(|json| !json.contains(&b'\n'))
        );
        assert!(
            matches!(serde_json::from_slice::<serde_json::Value>(&output),
            Ok(value) if value.get("number") == Some(&serde_json::json!(42))
                && value.get("repository") == Some(&serde_json::json!("owner/repo")))
        );
        let mut empty = Vec::new();
        assert!(write_next_json(&repository, &[], &mut empty).is_ok());
        assert_eq!(empty, b"null\n");
        let mut full = &mut [][..];
        assert!(matches!(
            write_next_json(&repository, &ranked, &mut full),
            Err(AppError::Output(error)) if error.kind() == io::ErrorKind::WriteZero
        ));
    }

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
        assert_eq!(scroll_by(ActionInput::Key(b'j'), 0, 9, 4), Some(1));
        assert_eq!(scroll_by(ActionInput::Down, 8, 9, 4), Some(9));
        assert_eq!(scroll_by(ActionInput::Key(b'k'), 0, 9, 4), Some(0));
        assert_eq!(
            parse_mouse_event("<0;20;5M"),
            Some(ActionInput::Mouse { x: 20, y: 5 })
        );
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
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[<35;20;5M")),
            Ok(ActionInput::Hover { x: 20, y: 5 })
        ));
        assert_eq!(
            action_for(ActionInput::Hover { x: 20, y: 5 }, &buttons, 0, 24),
            None
        );
        let raw = raw_terminal(unsafe { std::mem::zeroed() });
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG), 0);
        assert_eq!(raw.c_cc[libc::VMIN], 0);
        assert_eq!(raw.c_cc[libc::VTIME], 1);
        assert_eq!(
            TERMINAL_ENTER,
            "\x1b[?1049h\x1b[?1000h\x1b[?1003h\x1b[?1006h\x1b[?2004h"
        );
        assert_eq!(
            TERMINAL_RESTORE,
            "\x1b[?2004l\x1b[?1003l\x1b[?1000l\x1b[?1006l\x1b[?1049l"
        );
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
                &[],
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
        assert!(draw_view(&mut border_output, &[border.to_owned()], 0, 1, &[], &[]).is_ok());
        let border_output = String::from_utf8_lossy(&border_output);
        assert!(border_output.contains("\x1b[1;1H\x1b[2K┌────┐"));
        assert!(!border_output.contains("\x1b[K"));

        let mut search_output = Vec::new();
        let (header, _) = render_search_input("日本語", true, 40);
        assert!(
            draw_view(
                &mut search_output,
                &["table".to_owned()],
                0,
                2,
                &["help".to_owned()],
                &header,
            )
            .is_ok()
        );
        let search_output = String::from_utf8_lossy(&search_output);
        assert!(search_output.contains("\x1b[1;1H\x1b[2KKeywords> \x1b[1m[\x1b[22m 日本語"));
        assert!(
            search_output
                .contains("\x1b[2;1H\x1b[2K\x1b[7m Search \x1b[27m  Enter: search | Tab: list")
        );
        assert!(search_output.contains(&format!("\x1b[3;1H\x1b[2K{}", " ".repeat(40))));
        assert!(search_output.contains("\x1b[4;1H\x1b[2Ktable"));
        assert!(search_output.contains("\x1b[6;1H\x1b[2Khelp"));

        let no_input = read_input(&mut Cursor::new(b"")).unwrap_or(ActionInput::Quit);
        assert_eq!(no_input, ActionInput::Continue);
        assert!(!result_dismisses(no_input));
        assert!(result_dismisses(ActionInput::Key(b'x')));
    }

    #[test]
    fn parses_mouse_motion_and_rejects_invalid_events() {
        for button in 32..=35 {
            assert_eq!(
                parse_mouse_event(&format!("<{button};20;5M")),
                Some(ActionInput::Hover { x: 20, y: 5 })
            );
        }
        for (button, down) in [(64, false), (65, true)] {
            assert_eq!(
                parse_mouse_event(&format!("<{button};20;5M")),
                Some(ActionInput::Wheel { x: 20, y: 5, down })
            );
        }
        for invalid in ["<0;20;5m", "<66;20;5M", "<0;0;5M", "<0;20;0M", "<0;20;5;1M"] {
            assert_eq!(parse_mouse_event(invalid), None);
        }
    }

    #[test]
    fn list_layout_uses_physical_help_rows_and_keeps_the_header_and_footer() {
        let mut issues = sorting_issues();
        for issue in &mut issues {
            issue.issue.body = "body\n".repeat(30);
        }
        let table = render_search_results(&issues, 110, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(mut table) = table else { return };
        let mut output = Vec::new();
        let mut list = ListState::default();
        let search = SearchState::default();
        let help = vec!["help one".to_owned(), "help two".to_owned()];
        assert_eq!(
            draw_list_view(&mut output, &mut table, &mut list, &search, 20, 110, &help),
            Ok(14)
        );
        assert_eq!(
            table
                .issues
                .iter()
                .map(|row| row.lines.len())
                .collect::<Vec<_>>(),
            [5, 4, 4]
        );
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("\x1b[1;1H\x1b[2KKeywords:"));
        assert!(output.contains("\x1b[18;1H\x1b[2Khelp one"));
        assert!(output.contains("\x1b[19;1H\x1b[2Khelp two"));
        assert!(
            draw_list_view(
                &mut Vec::new(),
                &mut table,
                &mut list,
                &search,
                8,
                110,
                &help
            )
            .is_err()
        );
    }

    #[test]
    fn wheel_routes_to_the_scrolled_cell_without_changing_keyboard_actions() {
        let mut issues = sorting_issues();
        for issue in &mut issues {
            issue.issue.body = "body\n".repeat(30);
        }
        let table = render_search_results(&issues, 110, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(mut table) = table else { return };
        assert!(table.limit_rows(9).is_ok());
        let Some(region) = table.issues.get(1) else {
            return;
        };
        let line = region.lines.start;
        let scroll = line - 1;
        let mut search = SearchState::default();
        let input = search.route(ActionInput::Wheel {
            x: 109,
            y: SEARCH_ROWS + 2,
            down: true,
        });
        assert!(matches!(
            input,
            Ok(ActionInput::Wheel {
                x: 109,
                y: 2,
                down: true
            })
        ));
        let Ok(ActionInput::Wheel { x, y, down }) = input else {
            return;
        };
        table.scroll_cell(x, scroll + y - 1, down);
        assert!(table.limit_rows(9).is_ok());
        assert_eq!(hovered_issue(&table, x, y, scroll, 4, 110), Some(1));
        assert_eq!(
            action_for(ActionInput::Key(b'2'), &table.buttons, scroll, 4),
            Some(1)
        );
        let before = table.text.clone();
        assert!(navigate_issue(ActionInput::Down, &table, Some(1), scroll, 4).is_some());
        assert_eq!(
            scroll_by(ActionInput::PageDown, scroll, 100, 4),
            Some(scroll + 4)
        );
        assert_eq!(table.text, before);
        for _ in 0..100 {
            table.scroll_cell(x, line, true);
        }
        assert!(table.limit_rows(9).is_ok());
        assert!(table.buttons.iter().any(|button| {
            button.issue_index == 1
                && action_for(
                    ActionInput::Mouse {
                        x: button.x_start,
                        y: button.line - scroll + 1,
                    },
                    &table.buttons,
                    scroll,
                    100,
                ) == Some(1)
        }));
        let mut sorted = issues.to_vec();
        let mut list = ListState::default();
        list.sort_by(SortColumn::Number, &mut sorted);
        for visible in [sorted, search_issues(&issues, "issue 7")] {
            let reset = render_search_results(&visible, 110, &Ok(BTreeSet::new()), list.sort);
            assert!(reset.is_ok());
            let Ok(mut reset) = reset else { return };
            assert!(reset.limit_rows(9).is_ok());
            assert_eq!(reset.buttons, [] as [ButtonRegion; 0]);
            assert!(reset.text.contains("issue"));
        }
    }

    #[test]
    fn navigates_every_issue_even_when_the_table_fits_the_viewport() {
        let mut issues = [42, 43, 44].map(|number| model::RankedIssue {
            issue: model::Issue {
                details: Some(crate::model::test_issue_details()),
                number,
                title: format!("Issue {number}"),
                body: String::new(),
            },
            score: 3.6,
            prefix: "fix".to_owned(),
            readiness: crate::model::Readiness::Yes,
        });
        let table =
            render_table_with_buttons(&issues, 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        assert!(table.text.lines().count() < 100);
        let mut selected = None;
        for (input, expected) in [
            (ActionInput::Key(b'j'), 0),
            (ActionInput::Key(b'j'), 1),
            (ActionInput::Down, 2),
            (ActionInput::Down, 2),
            (ActionInput::Key(b'k'), 1),
            (ActionInput::Up, 0),
            (ActionInput::Up, 0),
        ] {
            let next = navigate_issue(input, &table, selected, 0, 100);
            assert_eq!(next, Some((expected, 0)));
            selected = next.map(|(index, _)| index);
        }
        for (index, issue) in table.issues.iter().enumerate() {
            let hovered = hovered_issue(&table, 20, issue.lines.start + 1, 0, 100, 100);
            assert_eq!(hovered, Some(index));
            assert_eq!(
                navigate_issue(ActionInput::Down, &table, hovered, 0, 100),
                Some(((index + 1).min(2), 0))
            );
        }
        assert_eq!(
            navigate_issue(ActionInput::Continue, &table, selected, 0, 100),
            None
        );
        for issue in &mut issues {
            issue.issue.body = "long body\n".repeat(12);
        }
        let tall =
            render_table_with_buttons(&issues, 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(tall.is_ok());
        let Ok(tall) = tall else { return };
        let mut selected = None;
        let mut scroll = 0;
        for expected in 0..3 {
            let next = navigate_issue(ActionInput::Key(b'j'), &tall, selected, scroll, 4);
            assert!(next.is_some());
            let Some((index, next_scroll)) = next else {
                return;
            };
            assert_eq!(index, expected);
            assert!(
                tall.issues
                    .get(index)
                    .is_some_and(|issue| issue.lines.start == next_scroll)
            );
            selected = Some(index);
            scroll = next_scroll;
        }
        let empty = render_table_with_buttons(&[], 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert_eq!(navigate_issue(ActionInput::Down, &empty, None, 0, 4), None);
    }

    #[test]
    fn page_keys_scroll_by_viewport_and_result_arrows_still_scroll_by_line() {
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[5~")),
            Ok(ActionInput::PageUp)
        ));
        assert!(matches!(
            read_input(&mut Cursor::new(b"\x1b[6~")),
            Ok(ActionInput::PageDown)
        ));
        assert_eq!(scroll_by(ActionInput::PageDown, 0, 9, 4), Some(4));
        assert_eq!(scroll_by(ActionInput::PageDown, 8, 9, 4), Some(9));
        assert_eq!(scroll_by(ActionInput::PageUp, 6, 9, 4), Some(2));
        assert_eq!(scroll_by(ActionInput::PageUp, 2, 9, 4), Some(0));
        assert_eq!(scroll_by(ActionInput::Down, 0, 9, 4), Some(1));
        assert_eq!(scroll_by(ActionInput::Key(b'k'), 6, 9, 4), Some(5));
    }

    #[test]
    fn locates_hovered_and_scrolled_issues() {
        let issues = [42, 43].map(|number| model::RankedIssue {
            issue: model::Issue {
                details: Some(crate::model::test_issue_details()),
                number,
                title: "タイトル🙂".to_owned(),
                body: "本文\n長い日本語の本文".to_owned(),
            },
            score: 3.6,
            prefix: "fix".to_owned(),
            readiness: crate::model::Readiness::Yes,
        });
        let table =
            render_table_with_buttons(&issues, 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        for (index, issue) in table.issues.iter().enumerate() {
            for line in issue.lines.clone() {
                assert_eq!(hovered_issue(&table, 20, 1, line, 4, 100), Some(index));
                assert_eq!(first_visible_issue(&table, line, 4), Some(index));
            }
            assert_eq!(hovered_issue(&table, 20, 1, issue.lines.end, 4, 100), None);
            assert_eq!(first_visible_issue(&table, issue.lines.start - 1, 1), None);
            assert_eq!(
                first_visible_issue(&table, issue.lines.start - 1, 2),
                Some(index)
            );
        }
        assert_eq!(hovered_issue(&table, 20, 1, 0, 4, 100), None);
        for (x, y) in [(0, 1), (101, 1), (20, 0), (20, 5)] {
            assert_eq!(hovered_issue(&table, x, y, 4, 4, 100), None);
        }
        assert_eq!(first_visible_issue(&table, 0, 4), None);
        assert_eq!(
            first_visible_issue(&table, table.text.lines().count(), 4),
            None
        );
        let empty = render_table_with_buttons(&[], 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert_eq!(first_visible_issue(&empty, 0, 24), None);
        assert_eq!(hovered_issue(&empty, 20, 1, 0, 24, 100), None);
    }
}
