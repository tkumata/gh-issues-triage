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
    CLOSE_BUTTON, CREATE_BUTTON, Dialog, DisplayError, HELP_TEXT, LIST_BUTTONS, RenderedTable,
    render_issue_table, render_next_json, render_search_input, render_search_results,
    search_button_start, terminal_height, terminal_width,
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
    let table = render_issue_table(&ranked, width, &branches, IssueSort::default())?;
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
    Continue,
    Quit,
}

#[derive(Default)]
struct SearchState {
    draft: String,
    query: String,
    pending_utf8: Vec<u8>,
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
    fn submit(&mut self) -> io::Result<()> {
        if !self.pending_utf8.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete UTF-8 input",
            ));
        }
        self.query.clone_from(&self.draft);
        Ok(())
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

    fn route(&mut self, input: ActionInput) -> io::Result<()> {
        match input {
            ActionInput::PasteByte(b'\t' | b'\r' | b'\n') => self.push_byte(b' ')?,
            ActionInput::Key(8 | 127) => {
                if self.pending_utf8.is_empty() {
                    self.draft.pop();
                } else {
                    self.pending_utf8.clear();
                }
            }
            ActionInput::Key(byte @ (32..=126 | 128..=255))
            | ActionInput::PasteByte(byte @ (32..=126 | 128..=255)) => self.push_byte(byte)?,
            _ => {}
        }
        Ok(())
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
                if self.escape == [0x1b] && !self.pasting {
                    self.escape.clear();
                    return Ok(ActionInput::Key(0x1b));
                }
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

fn wheel_scroll(input: ActionInput, scroll: usize, max_scroll: usize) -> usize {
    match input {
        ActionInput::Wheel { down: true, .. } => scroll.saturating_add(1).min(max_scroll),
        ActionInput::Wheel { down: false, .. } => scroll.saturating_sub(1),
        _ => scroll,
    }
}

fn list_action(input: ActionInput, height: usize) -> Option<u8> {
    match input {
        ActionInput::Quit => Some(b'q'),
        ActionInput::Key(key @ (b'h' | b'q' | b's')) => Some(key),
        ActionInput::Mouse { x, y } if y == height => {
            let mut start = 1;
            for (button, key) in LIST_BUTTONS.iter().zip(*b"hqs") {
                if (start..start + button.len()).contains(&x) {
                    return Some(key);
                }
                start += button.len() + 1;
            }
            None
        }
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

#[derive(Debug, PartialEq, Eq)]
enum DialogResult {
    Closed,
    Primary,
    Quit,
}

fn draw_dialog(stdout: &mut impl Write, dialog: &Dialog, scroll: usize) -> Result<(), String> {
    for (row, line) in dialog.lines(scroll).iter().enumerate() {
        write!(stdout, "\x1b[{};{}H{line}", dialog.top + row, dialog.left)
            .map_err(|error| error.to_string())?;
    }
    stdout.flush().map_err(|error| error.to_string())
}

fn show_dialog(
    dialog: &Dialog,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    input_state: &mut InputState,
) -> Result<DialogResult, String> {
    let mut scroll = 0;
    loop {
        draw_dialog(stdout, dialog, scroll)?;
        let input = input_state
            .read_input(stdin)
            .map_err(|error| error.to_string())?;
        match input {
            ActionInput::Quit => return Ok(DialogResult::Quit),
            ActionInput::Mouse { x, y } => match dialog.button_at(x, y) {
                Some(0) => return Ok(DialogResult::Closed),
                Some(_) => return Ok(DialogResult::Primary),
                None => {}
            },
            ActionInput::Wheel { x, y, .. } if dialog.content_contains(x, y) => {
                scroll = wheel_scroll(input, scroll, dialog.max_scroll());
            }
            _ => {}
        }
    }
}

fn show_search(
    search: &mut SearchState,
    (width, height): (usize, usize),
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    input_state: &mut InputState,
) -> Result<DialogResult, String> {
    let dialog = Dialog::search(width, height).map_err(|error| error.to_string())?;
    search.draft.clone_from(&search.query);
    search.pending_utf8.clear();
    loop {
        draw_dialog(stdout, &dialog, 0)?;
        let (line, cursor) = render_search_input(&search.draft, dialog.width - 4);
        write!(
            stdout,
            "\x1b[{};{}H{}\x1b[{};{}H",
            dialog.top + 3,
            dialog.left + 2,
            line,
            dialog.top + 3,
            dialog.left + 1 + cursor
        )
        .and_then(|()| stdout.flush())
        .map_err(|error| error.to_string())?;
        let input = input_state
            .read_input(stdin)
            .map_err(|error| error.to_string())?;
        match input {
            ActionInput::Quit => return Ok(DialogResult::Quit),
            ActionInput::Mouse { x, y } if dialog.button_at(x, y) == Some(0) => {
                search.draft.clone_from(&search.query);
                search.pending_utf8.clear();
                return Ok(DialogResult::Closed);
            }
            ActionInput::Mouse { x, y }
                if y == dialog.top + 3
                    && (dialog.left + 1 + search_button_start(dialog.width - 4)
                        ..dialog.left + dialog.width - 2)
                        .contains(&x) =>
            {
                search.submit().map_err(|error| error.to_string())?;
                return Ok(DialogResult::Primary);
            }
            _ => search.route(input).map_err(|error| error.to_string())?,
        }
    }
}

fn draw_list_view(
    stdout: &mut impl Write,
    table: &RenderedTable,
    list: &mut ListState,
    height: usize,
) -> Result<usize, String> {
    let mut footer = table.branch_errors.clone();
    footer.push(
        LIST_BUTTONS
            .iter()
            .map(|button| format!("\x1b[7m{button}\x1b[27m"))
            .collect::<Vec<_>>()
            .join(" "),
    );
    let viewport_rows = height.saturating_sub(footer.len());
    if viewport_rows == 0 {
        return Err("terminal is too short to show issue actions".to_owned());
    }
    let max_scroll = table.text.lines().count().saturating_sub(viewport_rows);
    list.scroll = list.scroll.min(max_scroll);
    draw_view(
        stdout,
        &table.lines(list.selected),
        list.scroll,
        viewport_rows,
        &footer,
        &[],
    )?;
    Ok(viewport_rows)
}

fn show_issue(
    create: impl FnOnce(&RankedIssue) -> Result<String, String>,
    issue: &RankedIssue,
    (width, height): (usize, usize),
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    input_state: &mut InputState,
) -> Result<DialogResult, String> {
    let title = format!("#{} {}", issue.issue.number, issue.issue.title);
    let body = format!(
        "{}\n\nBranch: {}",
        issue.issue.body,
        branch::branch_name(issue)
    );
    let dialog = Dialog::new(width, height, &title, &body, &[CLOSE_BUTTON, CREATE_BUTTON])
        .map_err(|error| error.to_string())?;
    let action = show_dialog(&dialog, stdin, stdout, input_state)?;
    if action != DialogResult::Primary {
        return Ok(action);
    }
    let result = create(issue);
    let succeeded = result.is_ok();
    let message = result.map_or_else(
        |error| format!("Branch creation failed: {error}"),
        |name| format!("Created branch {name}"),
    );
    let dialog = Dialog::new(width, height, "Branch creation", &message, &[CLOSE_BUTTON])
        .map_err(|error| error.to_string())?;
    if show_dialog(&dialog, stdin, stdout, input_state)? == DialogResult::Quit {
        Ok(DialogResult::Quit)
    } else if succeeded {
        Ok(DialogResult::Primary)
    } else {
        Ok(DialogResult::Closed)
    }
}

fn run_branch_actions(
    repository: &RepositoryRef,
    issues: &[RankedIssue],
    mut table: RenderedTable,
    mut branches: Result<BTreeSet<u64>, String>,
    width: usize,
) -> Result<(), String> {
    let height = terminal_height().map_err(|error| error.to_string())?;
    let mut terminal = TerminalMode::enter()?;
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    write!(stdout, "{TERMINAL_ENTER}").map_err(|error| error.to_string())?;
    let mut list = ListState::default();
    let mut search = SearchState::default();
    let mut input_state = InputState::default();
    let mut visible = issues.to_vec();
    loop {
        let viewport_rows = draw_list_view(&mut stdout, &table, &mut list, height)?;
        let input = input_state
            .read_input(&mut stdin)
            .map_err(|error| error.to_string())?;
        match list_action(input, height) {
            Some(b'q') => break,
            Some(b'h') => {
                let dialog = Dialog::new(width, height, "Help", HELP_TEXT, &[CLOSE_BUTTON])
                    .map_err(|error| error.to_string())?;
                if show_dialog(&dialog, &mut stdin, &mut stdout, &mut input_state)?
                    == DialogResult::Quit
                {
                    break;
                }
                continue;
            }
            Some(b's') => {
                match show_search(
                    &mut search,
                    (width, height),
                    &mut stdin,
                    &mut stdout,
                    &mut input_state,
                )? {
                    DialogResult::Quit => break,
                    DialogResult::Primary => {
                        visible = search_issues(issues, &search.query);
                        list.sort.apply(&mut visible);
                        table = render_search_results(&visible, width, &branches, list.sort)
                            .map_err(|error| error.to_string())?;
                        list.selected = None;
                        list.scroll = 0;
                    }
                    DialogResult::Closed => {}
                }
                continue;
            }
            _ => {}
        }
        if let ActionInput::Wheel { y, .. } = input {
            if (1..=viewport_rows).contains(&y) {
                list.scroll = wheel_scroll(
                    input,
                    list.scroll,
                    table.text.lines().count().saturating_sub(viewport_rows),
                );
                list.selected = None;
            }
            continue;
        }
        if let Some(column) = sort_column_for(input, &table, list.scroll, viewport_rows) {
            list.sort_by(column, &mut visible);
            table = render_search_results(&visible, width, &branches, list.sort)
                .map_err(|error| error.to_string())?;
            continue;
        }
        match input {
            ActionInput::Hover { x, y } => {
                list.selected = hovered_issue(&table, x, y, list.scroll, viewport_rows, width);
            }
            ActionInput::Mouse { x, y } => {
                let Some(issue) = hovered_issue(&table, x, y, list.scroll, viewport_rows, width)
                    .and_then(|index| visible.get(index))
                else {
                    continue;
                };
                match show_issue(
                    |issue| branch::create_branch(repository, issue),
                    issue,
                    (width, height),
                    &mut stdin,
                    &mut stdout,
                    &mut input_state,
                )? {
                    DialogResult::Quit => break,
                    DialogResult::Closed => {}
                    DialogResult::Primary => {
                        branches = branch::local_issue_branches(repository);
                        table = render_search_results(&visible, width, &branches, list.sort)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            _ => {}
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

    fn issues() -> Vec<RankedIssue> {
        [42, 7, 3]
            .map(|number| RankedIssue {
                issue: model::Issue {
                    number,
                    title: format!("issue {number}"),
                    body: format!("本文 {number}"),
                    details: Some(model::test_issue_details()),
                },
                score: 3.0,
                prefix: "feat".into(),
                readiness: model::Readiness::Yes,
            })
            .to_vec()
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
    fn footer_buttons_have_one_space_gaps_and_old_keys_do_nothing() {
        let mut start = 1;
        for (button, key) in LIST_BUTTONS.iter().zip(*b"hqs") {
            for x in start..start + button.len() {
                assert_eq!(list_action(ActionInput::Mouse { x, y: 24 }, 24), Some(key));
                assert_eq!(list_action(ActionInput::Mouse { x, y: 23 }, 24), None);
            }
            assert_eq!(
                list_action(
                    ActionInput::Mouse {
                        x: start + button.len(),
                        y: 24
                    },
                    24
                ),
                None
            );
            assert_eq!(list_action(ActionInput::Key(key), 24), Some(key));
            start += button.len() + 1;
        }
        for key in b"jk0123456789\t\r\x1b" {
            assert_eq!(list_action(ActionInput::Key(*key), 24), None);
            assert_eq!(wheel_scroll(ActionInput::Key(*key), 4, 10), 4);
        }
        for sequence in [b"\x1b[A".as_slice(), b"\x1b[B", b"\x1b[5~", b"\x1b[6~"] {
            assert_eq!(
                InputState::default()
                    .read_input(&mut Cursor::new(sequence))
                    .ok(),
                Some(ActionInput::Continue)
            );
        }
        assert_eq!(
            InputState::default().read_input(&mut Cursor::new([3])).ok(),
            Some(ActionInput::Quit)
        );
    }

    #[test]
    fn search_accepts_utf8_and_paste_but_never_executes_shortcuts() {
        let mut search = SearchState::default();
        for byte in "日本語 hqs".bytes() {
            assert!(search.route(ActionInput::Key(byte)).is_ok());
        }
        for key in [b'\t', b'\r', 0x1b] {
            assert!(search.route(ActionInput::Key(key)).is_ok());
        }
        assert_eq!(search.draft, "日本語 hqs");
        assert_eq!(search.query, "");
        assert!(search.route(ActionInput::Key(127)).is_ok());
        assert_eq!(search.draft, "日本語 hq");
        for byte in b"\tq\nh" {
            assert!(search.route(ActionInput::PasteByte(*byte)).is_ok());
        }
        assert_eq!(search.draft, "日本語 hq q h");
        assert!(search.submit().is_ok());
        assert_eq!(search.query, search.draft);
        assert!(search.push_byte(0xe6).is_ok());
        assert!(search.submit().is_err());
        search.pending_utf8.clear();
        assert!(search.push_byte(0xff).is_err());
        search.pending_utf8.clear();
        assert!(search.push_byte(0x1b).is_err());
    }

    #[test]
    fn modal_ignores_keys_and_background_clicks_until_close_or_primary() {
        let dialog = Dialog::new(
            100,
            24,
            "本文",
            &"長文\n".repeat(100),
            &[CLOSE_BUTTON, CREATE_BUTTON],
        )
        .map_err(|e| e.to_string());
        assert!(dialog.is_ok());
        let Ok(dialog) = dialog else {
            return;
        };
        let mut input = Cursor::new(format!(
            "hqsjk\r\x1b[<0;1;24M\x1b[<0;{};{}Mtail",
            dialog.left + 2 + CLOSE_BUTTON.len() + 1,
            dialog.button_y
        ));
        let mut output = Vec::new();
        let mut state = InputState::default();
        assert_eq!(
            show_dialog(&dialog, &mut input, &mut output, &mut state).ok(),
            Some(DialogResult::Primary)
        );
        assert_eq!(
            state.read_input(&mut input).ok(),
            Some(ActionInput::Key(b't'))
        );
        let mut input = Cursor::new(format!("\x1b[<0;{};{}M", dialog.left + 2, dialog.button_y));
        assert_eq!(
            show_dialog(&dialog, &mut input, &mut output, &mut state).ok(),
            Some(DialogResult::Closed)
        );
        assert_eq!(
            show_dialog(&dialog, &mut Cursor::new([3]), &mut output, &mut state).ok(),
            Some(DialogResult::Quit)
        );
        let mut full = &mut [][..];
        assert!(show_dialog(&dialog, &mut Cursor::new([3]), &mut full, &mut state).is_err());
    }

    #[test]
    fn search_dialog_applies_only_on_click_and_close_discards_draft() {
        let dialog = Dialog::search(100, 24).map_err(|e| e.to_string());
        assert!(dialog.is_ok());
        let Ok(dialog) = dialog else {
            return;
        };
        let mut search = SearchState {
            query: "本文".into(),
            ..SearchState::default()
        };
        let submit_x = dialog.left + 1 + search_button_start(dialog.width - 4);
        let mut input = Cursor::new(format!(" 7\r\x1b[<0;{submit_x};{}M", dialog.top + 3));
        let mut output = Vec::new();
        assert_eq!(
            show_search(
                &mut search,
                (100, 24),
                &mut input,
                &mut output,
                &mut InputState::default()
            )
            .ok(),
            Some(DialogResult::Primary)
        );
        assert_eq!(search.query, "本文 7");
        assert_eq!(
            search_issues(&issues(), &search.query)
                .iter()
                .map(|i| i.issue.number)
                .collect::<Vec<_>>(),
            [7]
        );
        let mut input = Cursor::new(format!(
            "abc\x1b[<0;{};{}M",
            dialog.left + 2,
            dialog.button_y
        ));
        assert_eq!(
            show_search(
                &mut search,
                (100, 24),
                &mut input,
                &mut output,
                &mut InputState::default()
            )
            .ok(),
            Some(DialogResult::Closed)
        );
        assert_eq!(search.draft, "本文 7");
        assert_eq!(search.query, "本文 7");
        search.draft.clear();
        assert!(search.submit().is_ok());
        assert_eq!(search_issues(&issues(), &search.query).len(), 3);
    }

    #[test]
    fn issue_dialog_creates_only_the_displayed_issue_and_reports_success_or_failure() {
        let issue = issues().pop();
        assert!(issue.is_some());
        let Some(issue) = issue else {
            return;
        };
        let dialog = Dialog::new(100, 24, "", "", &[CLOSE_BUTTON, CREATE_BUTTON]);
        assert!(dialog.is_ok());
        let Ok(dialog) = dialog else {
            return;
        };
        let close = format!("\x1b[<0;{};{}M", dialog.left + 2, dialog.button_y);
        let create = format!(
            "\x1b[<0;{};{}M",
            dialog.left + 3 + CLOSE_BUTTON.len(),
            dialog.button_y
        );
        for succeeded in [false, true] {
            let mut target = None;
            let mut output = Vec::new();
            let mut input = Cursor::new(format!("{create}{close}"));
            let result = show_issue(
                |displayed| {
                    target = Some(displayed.issue.number);
                    if succeeded {
                        Ok(branch::branch_name(displayed))
                    } else {
                        Err("test Git failure".into())
                    }
                },
                &issue,
                (100, 24),
                &mut input,
                &mut output,
                &mut InputState::default(),
            );
            assert_eq!(target, Some(issue.issue.number));
            assert_eq!(
                result.ok(),
                Some(if succeeded {
                    DialogResult::Primary
                } else {
                    DialogResult::Closed
                })
            );
            let output = String::from_utf8_lossy(&output);
            assert!(output.contains(&issue.issue.title));
            assert!(output.contains(&issue.issue.body));
            assert!(output.contains(if succeeded {
                "Created branch feat/issue-3"
            } else {
                "Branch creation failed: test Git failure"
            }));
        }
        let mut invoked = false;
        let result = show_issue(
            |_| {
                invoked = true;
                Ok(String::new())
            },
            &issue,
            (100, 24),
            &mut Cursor::new(close),
            &mut Vec::new(),
            &mut InputState::default(),
        );
        assert_eq!(result.ok(), Some(DialogResult::Closed));
        assert!(!invoked);
    }

    #[test]
    fn dialog_wheel_is_local_and_clamped_and_paste_survives_timeouts() {
        let dialog = Dialog::new(100, 24, "Help", "first\nsecond\nthird", &[CLOSE_BUTTON]);
        assert!(dialog.is_ok());
        let Ok(dialog) = dialog else {
            return;
        };
        let text = (0..30)
            .map(|i| format!("row {i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let long = Dialog::new(100, 24, "Help", &text, &[CLOSE_BUTTON]);
        assert!(long.is_ok());
        let Ok(long) = long else {
            return;
        };
        let mut events = format!("\x1b[<65;1;1M\x1b[<0;{};{}M", long.left + 2, long.button_y);
        let mut output = Vec::new();
        let mut state = InputState::default();
        assert_eq!(
            show_dialog(&long, &mut Cursor::new(&events), &mut output, &mut state).ok(),
            Some(DialogResult::Closed)
        );
        let output = String::from_utf8_lossy(&output);
        assert!(!output.contains("row 14"));
        events = format!(
            "{}{}\x1b[<0;{};{}M",
            format!("\x1b[<65;{};{}M", long.left + 2, long.top + 3).repeat(60),
            format!("\x1b[<64;{};{}M", long.left + 2, long.top + 3).repeat(60),
            long.left + 2,
            long.button_y
        );
        let mut output = Vec::new();
        assert_eq!(
            show_dialog(&long, &mut Cursor::new(events), &mut output, &mut state).ok(),
            Some(DialogResult::Closed)
        );
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("row 29"));
        assert!(output.rfind("row 00") > output.rfind("row 29"));
        assert_eq!(dialog.max_scroll(), 0);
        assert_eq!(
            state.read_input(&mut Cursor::new(b"\x1b[20")).ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(
            state.read_input(&mut Cursor::new(b"0~")).ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(
            state.read_input(&mut Cursor::new(b"q")).ok(),
            Some(ActionInput::PasteByte(b'q'))
        );
        assert_eq!(
            state.read_input(&mut Cursor::new(b"\x1b[<0;1;24M")).ok(),
            Some(ActionInput::Continue)
        );
        assert_eq!(
            state.read_input(&mut Cursor::new(b"\x1b[201~")).ok(),
            Some(ActionInput::Continue)
        );
        assert!(!state.pasting);
        let raw = raw_terminal(unsafe { std::mem::zeroed() });
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG), 0);
        assert_eq!(raw.c_cc[libc::VMIN], 0);
        assert_eq!(raw.c_cc[libc::VTIME], 1);
    }

    #[test]
    fn sorted_filtered_rows_keep_hover_click_and_branch_targets_together() {
        let mut visible = search_issues(&issues(), "本文");
        let mut list = ListState {
            selected: Some(1),
            scroll: 4,
            ..ListState::default()
        };
        list.sort_by(SortColumn::Number, &mut visible);
        assert_eq!(list.selected, None);
        assert_eq!(list.scroll, 4);
        let table = render_issue_table(&visible, 110, &Ok(BTreeSet::new()), list.sort)
            .map_err(|e| e.to_string());
        assert!(table.is_ok());
        let Ok(table) = table else {
            return;
        };
        for (index, row) in table.issues.iter().enumerate() {
            assert_eq!(
                hovered_issue(&table, 10, 1, row.lines.start, 5, 110),
                Some(index)
            );
            let issue = visible.get(index);
            assert!(issue.is_some());
            let Some(issue) = issue else {
                return;
            };
            assert_eq!(
                branch::branch_name(issue),
                format!("feat/issue-{}", issue.issue.number)
            );
            assert_eq!(hovered_issue(&table, 10, 1, row.lines.end, 5, 110), None);
        }
        for header in &table.sort_headers {
            assert_eq!(
                sort_column_for(
                    ActionInput::Mouse {
                        x: header.x_start,
                        y: header.line + 1
                    },
                    &table,
                    0,
                    24
                ),
                Some(header.column)
            );
            assert_eq!(
                sort_column_for(
                    ActionInput::Mouse {
                        x: header.x_end + 1,
                        y: header.line + 1
                    },
                    &table,
                    0,
                    24
                ),
                None
            );
        }
        let mut output = Vec::new();
        assert_eq!(
            draw_list_view(&mut output, &table, &mut list, 24).ok(),
            Some(23)
        );
        let output = String::from_utf8(output).map_err(|e| e.to_string());
        assert!(output.is_ok());
        let Ok(output) = output else {
            return;
        };
        assert!(output.contains("\x1b[24;1H"));
        assert!(!output.contains("Keywords"));
        assert_eq!(
            wheel_scroll(
                ActionInput::Wheel {
                    x: 1,
                    y: 1,
                    down: true
                },
                10,
                10
            ),
            10
        );
        assert_eq!(
            wheel_scroll(
                ActionInput::Wheel {
                    x: 1,
                    y: 1,
                    down: false
                },
                0,
                10
            ),
            0
        );
    }
}
