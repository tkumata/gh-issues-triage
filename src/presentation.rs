use std::collections::BTreeSet;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::{IssueSort, RankedIssue, RepositoryRef, SortColumn};

pub(crate) fn render_next_json(
    repository: &RepositoryRef,
    selected: Option<&RankedIssue>,
) -> Result<String, serde_json::Error> {
    let value = selected.map(|ranked| {
        serde_json::json!({
            "repository": format!("{}/{}", repository.owner, repository.repo),
            "number": ranked.issue.number,
            "title": ranked.issue.title,
            "body": ranked.issue.body,
            "score": ranked.score,
            "readiness": ranked.readiness.label(),
            "branch_name": crate::branch::branch_name(ranked),
        })
    });
    serde_json::to_string(&value)
}

#[derive(Debug)]
pub(crate) enum DisplayError {
    TerminalWidthUnavailable,
    TerminalWidthTooSmall { width: usize },
    MissingIssueDetails,
    TerminalHeightTooSmall,
}

pub(crate) struct SortRegion {
    pub(crate) column: SortColumn,
    pub(crate) x_start: usize,
    pub(crate) x_end: usize,
    pub(crate) line: usize,
}

pub(crate) const SEARCH_BUTTON: &str = "   Search   ";

pub(crate) const LIST_BUTTONS: [&str; 3] = ["   HELP   ", "   QUIT   ", "   SEARCH   "];
pub(crate) const CREATE_BUTTON: &str = " Create branch ";
pub(crate) const CLOSE_BUTTON: &str = " Close ";

pub(crate) fn search_button_start(width: usize) -> usize {
    width.saturating_sub(SEARCH_BUTTON.len()) + 1
}

pub(crate) fn render_search_input(query: &str, width: usize) -> (String, usize) {
    let label = "Keywords> ";
    let field_width = width.saturating_sub(label.width() + 5 + SEARCH_BUTTON.len());
    let available = field_width.saturating_sub(1);
    let mut tail = "";
    for (start, _) in query.char_indices().rev() {
        let candidate = query.split_at(start).1;
        if candidate.width() > available {
            break;
        }
        tail = candidate;
    }
    let placeholder = "Type keywords...";
    let content = if query.is_empty() {
        &placeholder[..placeholder.len().min(available)]
    } else {
        tail
    };
    let (left, right) = ("\x1b[1m[\x1b[22m", "\x1b[1m]\x1b[22m");
    let padding = " ".repeat(field_width.saturating_sub(content.width()));
    let content = if query.is_empty() {
        format!("\x1b[90m{content}\x1b[39m")
    } else {
        content.to_owned()
    };
    let text = format!("{label}{left} {content}{padding} {right} \x1b[7m{SEARCH_BUTTON}\x1b[27m");
    let cursor = label.width() + 3 + tail.width();
    (text, cursor)
}

pub(crate) const HELP_TEXT: &str = "一覧画面の操作

HELP / h: ヘルプを開く
QUIT / q: アプリを終了
SEARCH / s: 検索を開く
マウスオーバー: Issue を仮選択
Issue 行をクリック: 本文とブランチ作成ボタンを表示
マウスホイール: 一覧全体をスクロール
重要度・番号・Author・日付の見出し: 並べ替え

ダイアログの操作

Close: 一覧へ戻る
本文とヘルプの内容: マウスホイールでスクロール
Create branch: 表示中の Issue のブランチを作成
Ctrl+C: アプリを終了

検索方法

入力欄に検索語を入力し、Search をクリック
Backspace: 末尾の文字を削除
空白区切りの AND 検索。語順は不問
タイトルと本文を検索。ASCII 英字の大小は区別しない
空欄・空白だけの検索: 絞り込みを解除
Close: 入力した検索語を破棄して戻る

表示の意味

重要度: 0 (低) ～ 4 (高)
番号 + 🌿: ブランチあり
番号のみ: ブランチなし
番号 + ?: 確認不能
Ready: 仕様整理への着手可能度
Yes: 着手可能
Needs information: 情報の追加が必要
Needs investigation: 事前調査が必要";

pub(crate) struct Dialog {
    pub(crate) left: usize,
    pub(crate) top: usize,
    pub(crate) rows: usize,
    pub(crate) button_y: usize,
    pub(crate) width: usize,
    title: String,
    buttons: Vec<&'static str>,
    content: Vec<String>,
}

impl Dialog {
    pub(crate) fn new(
        terminal_width: usize,
        terminal_height: usize,
        title: &str,
        text: &str,
        buttons: &[&'static str],
    ) -> Result<Self, DisplayError> {
        Self::with_height(
            terminal_width,
            terminal_height,
            terminal_height.saturating_sub(4),
            title,
            text,
            buttons,
        )
    }

    pub(crate) fn search(width: usize, height: usize) -> Result<Self, DisplayError> {
        let dialog = Self::with_height(width, height, 7, "Search", "", &[CLOSE_BUTTON])?;
        if dialog.width < 37 {
            return Err(DisplayError::TerminalWidthTooSmall { width });
        }
        Ok(dialog)
    }

    fn with_height(
        terminal_width: usize,
        terminal_height: usize,
        height: usize,
        title: &str,
        text: &str,
        buttons: &[&'static str],
    ) -> Result<Self, DisplayError> {
        let width = terminal_width * 3 / 4;
        let buttons_width = buttons.iter().map(|button| button.len()).sum::<usize>()
            + buttons.len().saturating_sub(1);
        if width < 30 || width < buttons_width + 4 {
            return Err(DisplayError::TerminalWidthTooSmall {
                width: terminal_width,
            });
        }
        if height < 7 || height > terminal_height {
            return Err(DisplayError::TerminalHeightTooSmall);
        }
        let top = (terminal_height - height) / 2 + 1;
        Ok(Self {
            left: (terminal_width - width) / 2 + 1,
            top,
            rows: height - 6,
            button_y: top + height - 2,
            width,
            title: wrap_line(title, width - 4)
                .into_iter()
                .next()
                .unwrap_or_default(),
            buttons: buttons.to_vec(),
            content: text
                .lines()
                .flat_map(|line| wrap_line(line, width - 4))
                .collect(),
        })
    }

    pub(crate) fn max_scroll(&self) -> usize {
        self.content.len().saturating_sub(self.rows)
    }

    pub(crate) fn button_at(&self, x: usize, y: usize) -> Option<usize> {
        if y != self.button_y {
            return None;
        }
        let mut start = self.left + 2;
        for (index, button) in self.buttons.iter().enumerate() {
            if (start..start + button.len()).contains(&x) {
                return Some(index);
            }
            start += button.len() + 1;
        }
        None
    }

    pub(crate) fn content_contains(&self, x: usize, y: usize) -> bool {
        (self.left + 1..self.left + self.width - 1).contains(&x)
            && (self.top + 3..self.top + 3 + self.rows).contains(&y)
    }

    pub(crate) fn lines(&self, scroll: usize) -> Vec<String> {
        let mut lines = vec![format!("┌{}┐", "─".repeat(self.width - 2))];
        lines.push(format!("│ {} │", pad_cell(&self.title, self.width - 4)));
        lines.push(format!("│{}│", pad_cell("", self.width - 2)));
        for row in 0..self.rows {
            let text = self.content.get(scroll + row).map_or("", String::as_str);
            lines.push(format!("│ {} │", pad_cell(text, self.width - 4)));
        }
        lines.push(format!("│{}│", pad_cell("", self.width - 2)));
        let buttons = self
            .buttons
            .iter()
            .map(|button| format!("\x1b[7m{button}\x1b[27m"))
            .collect::<Vec<_>>()
            .join(" ");
        let buttons_width = self
            .buttons
            .iter()
            .map(|button| button.len())
            .sum::<usize>()
            + self.buttons.len().saturating_sub(1);
        lines.push(format!(
            "│ {buttons}{}│",
            " ".repeat(self.width - 3 - buttons_width)
        ));
        lines.push(format!("└{}┘", "─".repeat(self.width - 2)));
        lines
    }
}

pub(crate) fn render_search_results(
    issues: &[RankedIssue],
    width: usize,
    branches: &Result<BTreeSet<u64>, String>,
    sort: IssueSort,
) -> Result<RenderedTable, DisplayError> {
    let mut table = render_issue_table(issues, width, branches, sort)?;
    if issues.is_empty() {
        table.text.push('\n');
        table.text.push_str(&pad_cell("No matching issues.", width));
    }
    Ok(table)
}

pub(crate) struct RenderedTable {
    pub(crate) text: String,
    pub(crate) issues: Vec<IssueRegion>,
    pub(crate) branch_errors: Vec<String>,
    pub(crate) sort_headers: Vec<SortRegion>,
}

pub(crate) struct IssueRegion {
    pub(crate) lines: std::ops::Range<usize>,
    highlighted: Vec<String>,
}

impl RenderedTable {
    pub(crate) fn lines(&self, highlighted: Option<usize>) -> Vec<String> {
        let mut lines = self.text.lines().map(str::to_owned).collect::<Vec<_>>();
        if let Some(issue) = highlighted.and_then(|index| self.issues.get(index)) {
            for (line_index, highlighted_line) in issue.lines.clone().zip(&issue.highlighted) {
                if let Some(line) = lines.get_mut(line_index) {
                    line.clone_from(highlighted_line);
                }
            }
        }
        lines
    }
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TerminalWidthUnavailable => formatter.write_str("terminal width is unavailable"),
            Self::MissingIssueDetails => formatter.write_str("issue display details are missing"),
            Self::TerminalHeightTooSmall => {
                formatter.write_str("terminal is too short to show issue rows")
            }
            Self::TerminalWidthTooSmall { width } => write!(
                formatter,
                "terminal width {width} is too small for the table"
            ),
        }
    }
}

#[cfg(unix)]
pub(crate) fn terminal_width() -> Result<usize, DisplayError> {
    terminal_dimensions().map(|(width, _)| width)
}

#[cfg(unix)]
pub(crate) fn terminal_height() -> Result<usize, DisplayError> {
    terminal_dimensions().map(|(_, height)| height)
}

#[cfg(unix)]
fn terminal_dimensions() -> Result<(usize, usize), DisplayError> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
    if result != 0 || size.ws_col == 0 || size.ws_row == 0 {
        return Err(DisplayError::TerminalWidthUnavailable);
    }
    Ok((usize::from(size.ws_col), usize::from(size.ws_row)))
}

#[cfg(not(unix))]
pub(crate) fn terminal_width() -> Result<usize, DisplayError> {
    Err(DisplayError::TerminalWidthUnavailable)
}

#[cfg(not(unix))]
pub(crate) fn terminal_height() -> Result<usize, DisplayError> {
    Err(DisplayError::TerminalWidthUnavailable)
}

pub(crate) fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in line.chars().map(|character| {
        if character.is_control() {
            ' '
        } else {
            character
        }
    }) {
        let character_width = character.width().unwrap_or(0);
        if !current.is_empty() && current_width + character_width > width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(character);
        current_width += character_width;
    }
    lines.push(current);
    lines
}

fn pad_cell(value: &str, width: usize) -> String {
    format!("{value}{}", " ".repeat(width.saturating_sub(value.width())))
}

fn horizontal_rule(left: char, junction: char, right: char, widths: [usize; 7]) -> String {
    let cells = widths.map(|width| "─".repeat(width + 2));
    format!("{left}{}{right}", cells.join(&junction.to_string()))
}

fn table_row(cells: [&str; 7], widths: [usize; 7], highlighted: bool) -> String {
    let cells = cells.into_iter().zip(widths).map(|(cell, width)| {
        let padded = format!(" {} ", pad_cell(cell, width));
        if highlighted {
            format!("\x1b[30;104m{padded}\x1b[39;49m")
        } else {
            padded
        }
    });
    format!("│{}│", cells.collect::<Vec<_>>().join("│"))
}

fn branch_number_cell(
    number: u64,
    label_width: usize,
    branches: &Result<BTreeSet<u64>, String>,
) -> String {
    let status = match branches {
        Ok(numbers) if numbers.contains(&number) => "🌿",
        Ok(_) => "",
        Err(_) => "?",
    };
    format!("{} {status}", pad_cell(&format!("#{number}"), label_width))
}

#[cfg(test)]
fn render_table(issues: &[RankedIssue], width: usize) -> Result<String, DisplayError> {
    render_issue_table(issues, width, &Ok(BTreeSet::new()), IssueSort::default())
        .map(|table| table.text)
}

fn table_widths(issues: &[RankedIssue], width: usize) -> Result<[usize; 7], DisplayError> {
    let importance_width = issues
        .iter()
        .map(|issue| issue.score.to_string().width())
        .max()
        .unwrap_or(0)
        .max("重要度↓".width());
    let ready_width = issues
        .iter()
        .map(|issue| issue.readiness.label().width())
        .max()
        .unwrap_or(0)
        .max("Ready".width());
    let number_label_width = issues
        .iter()
        .map(|issue| format!("#{}", issue.issue.number).width())
        .max()
        .unwrap_or(0)
        .max("番号".width());
    let mut number_width = number_label_width;
    if !issues.is_empty() {
        number_width += 1 + "🌿".width();
    }
    number_width = number_width.max("番号↓".width());
    let date_width = "YYYY-MM-DD".width();
    let fixed_width = importance_width + ready_width + number_width + date_width + 22;
    let minimum_variable = "Author↓".width() + "Assignees".width() + "Issues".width();
    if width < fixed_width + minimum_variable {
        return Err(DisplayError::TerminalWidthTooSmall { width });
    }
    let spare = width - fixed_width - minimum_variable;
    let author_width = issues
        .iter()
        .filter_map(|issue| issue.issue.details.as_ref())
        .filter_map(|details| details.author.as_deref())
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
        .clamp("Author↓".width(), "Author↓".width() + spare / 4);
    let assignees_width = issues
        .iter()
        .filter_map(|issue| issue.issue.details.as_ref())
        .map(|details| details.assignees.join(", ").width())
        .max()
        .unwrap_or(0)
        .clamp("Assignees".width(), "Assignees".width() + spare / 4);
    Ok([
        importance_width,
        ready_width,
        number_width,
        author_width,
        assignees_width,
        date_width,
        width - fixed_width - author_width - assignees_width,
    ])
}

fn table_header(widths: [usize; 7], sort: IssueSort, line: usize) -> (String, Vec<SortRegion>) {
    let headers = [
        ("重要度", Some(SortColumn::Importance)),
        ("Ready", None),
        ("番号", Some(SortColumn::Number)),
        ("Author", Some(SortColumn::Author)),
        ("Assignees", None),
        ("日付", Some(SortColumn::CreatedAt)),
        ("Issues", None),
    ];
    let labels = headers.map(|(label, column)| {
        if column == Some(sort.column) {
            format!("{label}{}", if sort.descending { "↓" } else { "↑" })
        } else {
            label.to_owned()
        }
    });
    let mut x_start = 2;
    let mut sort_headers = Vec::new();
    for ((_, column), width) in headers.into_iter().zip(widths) {
        if let Some(column) = column {
            sort_headers.push(SortRegion {
                column,
                x_start,
                x_end: x_start + width + 1,
                line,
            });
        }
        x_start += width + 3;
    }
    (
        table_row(labels.each_ref().map(String::as_str), widths, false),
        sort_headers,
    )
}

fn render_issue(
    ranked: &RankedIssue,
    details: &crate::model::IssueDetails,
    widths: [usize; 7],
    branches: &Result<BTreeSet<u64>, String>,
    first_line: usize,
) -> (Vec<String>, IssueRegion) {
    let assignees = if details.assignees.is_empty() {
        "-".to_owned()
    } else {
        details.assignees.join(", ")
    };
    let cells = [
        vec![ranked.score.to_string()],
        vec![ranked.readiness.label().to_owned()],
        vec![branch_number_cell(
            ranked.issue.number,
            widths[2] - 1 - "🌿".width(),
            branches,
        )],
        wrap_line(details.author.as_deref().unwrap_or("-"), widths[3]),
        wrap_line(&assignees, widths[4]),
        vec![details.created_at.jst_date.clone()],
        ranked
            .issue
            .title
            .lines()
            .flat_map(|line| wrap_line(line, widths[6]))
            .collect(),
    ];
    let height = cells.iter().map(Vec::len).max().unwrap_or(0);
    let mut rows = Vec::new();
    let mut highlighted = Vec::new();
    for row in 0..height {
        let values = cells
            .each_ref()
            .map(|cell| cell.get(row).map_or("", String::as_str));
        rows.push(table_row(values, widths, false));
        highlighted.push(table_row(values, widths, true));
    }
    (
        rows,
        IssueRegion {
            lines: first_line..first_line + height,
            highlighted,
        },
    )
}

pub(crate) fn render_issue_table(
    issues: &[RankedIssue],
    width: usize,
    branches: &Result<BTreeSet<u64>, String>,
    sort: IssueSort,
) -> Result<RenderedTable, DisplayError> {
    let details = issues
        .iter()
        .map(|issue| {
            issue
                .issue
                .details
                .as_ref()
                .ok_or(DisplayError::MissingIssueDetails)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let widths = table_widths(issues, width)?;
    let mut lines = wrap_line("重要度: 0（低）〜4（高）", width)
        .into_iter()
        .map(|line| pad_cell(&line, width))
        .collect::<Vec<_>>();
    lines.push(horizontal_rule('┌', '┬', '┐', widths));
    let (header, sort_headers) = table_header(widths, sort, lines.len());
    lines.push(header);
    if issues.is_empty() {
        lines.push(horizontal_rule('└', '┴', '┘', widths));
        return Ok(RenderedTable {
            text: lines.join("\n"),
            issues: Vec::new(),
            branch_errors: branch_errors(branches, width),
            sort_headers,
        });
    }
    lines.push(horizontal_rule('├', '┼', '┤', widths));
    let mut issue_regions = Vec::new();
    for (issue_index, (ranked, details)) in issues.iter().zip(details).enumerate() {
        let (rows, region) = render_issue(ranked, details, widths, branches, lines.len());
        lines.extend(rows);
        issue_regions.push(region);
        if issue_index + 1 < issues.len() {
            lines.push(horizontal_rule('├', '┼', '┤', widths));
        }
    }
    lines.push(horizontal_rule('└', '┴', '┘', widths));
    debug_assert!(lines.iter().all(|line| line.width() == width));
    Ok(RenderedTable {
        text: lines.join("\n"),
        issues: issue_regions,
        branch_errors: branch_errors(branches, width),
        sort_headers,
    })
}

fn branch_errors(branches: &Result<BTreeSet<u64>, String>, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if let Err(reason) = branches {
        for line in format!("ブランチ確認不能: {reason}").lines() {
            lines.extend(wrap_line(line, width));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_wrapped_metadata_jst_and_sort_directions_without_control_sequences() {
        let mut ranked = issue(42, "title", "body");
        let mut details = crate::model::test_issue_details();
        details.author = Some("long_author_\x1b[31m日本語".into());
        details.assignees = vec![
            "alice".into(),
            "long_assignee_\x1b[31m".into(),
            "zoe".into(),
        ];
        ranked.issue.details = Some(details);
        let sort = IssueSort {
            column: SortColumn::Author,
            descending: false,
        };
        let table = render_issue_table(&[ranked], 100, &Ok(BTreeSet::new()), sort);
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        assert!(table.text.contains("Author↑"));
        assert!(table.text.contains("2026-10-23"));
        assert!(table.text.lines().all(|line| line.width() == 100));
        assert!(
            !table
                .text
                .chars()
                .any(|character| character.is_control() && character != '\n')
        );
        let mut author = String::new();
        let mut assignees = String::new();
        for region in &table.issues {
            for line in table
                .text
                .lines()
                .skip(region.lines.start)
                .take(region.lines.len())
            {
                author.push_str(line.split('│').nth(4).unwrap_or("").trim());
                assignees.push_str(line.split('│').nth(5).unwrap_or("").trim());
            }
        }
        assert_eq!(author.replace(' ', ""), "long_author_[31m日本語");
        assert_eq!(assignees.replace(' ', ""), "alice,long_assignee_[31m,zoe");
        let unassigned = render_issue_table(
            &[issue(1, "title", "")],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(unassigned.is_ok());
        let Ok(unassigned) = unassigned else { return };
        assert!(unassigned.text.contains("重要度↓"));
        assert!(
            unassigned
                .issues
                .first()
                .and_then(|region| unassigned.text.lines().nth(region.lines.start))
                .and_then(|line| line.split('│').nth(5))
                .is_some_and(|cell| cell.trim() == "-")
        );
    }

    #[test]
    fn search_field_keeps_unicode_tail_placeholder_and_cursor_in_bounds() {
        for width in [33, 40, 60, 100] {
            for query in [
                String::new(),
                " ".into(),
                "日本🙂".into(),
                format!("{}末尾🙂", "長い検索語".repeat(20)),
            ] {
                let (line, cursor) = render_search_input(&query, width);
                let visible = line
                    .replace("\x1b[7m", "")
                    .replace("\x1b[27m", "")
                    .replace("\x1b[1m", "")
                    .replace("\x1b[22m", "")
                    .replace("\x1b[90m", "")
                    .replace("\x1b[39m", "");
                assert_eq!(visible.width(), width);
                assert!(!visible.chars().any(char::is_control));
                assert!(visible.starts_with("Keywords> [ "));
                assert!(visible.ends_with(SEARCH_BUTTON));
                assert_eq!(line.contains("\x1b[90m"), query.is_empty());
                assert!(cursor < search_button_start(width));
                if query.ends_with("末尾🙂") && width >= 40 {
                    assert!(visible.contains("末尾🙂"));
                }
                if query.is_empty() {
                    assert_eq!(cursor, 13);
                }
            }
        }
    }

    #[test]
    fn renders_json_without_terminal_sanitizing_or_score_rounding() {
        let repository = RepositoryRef {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        for body in ["", "本文\n\"引用\"\t\u{1b}"] {
            for (prefix, branch_name) in [
                ("refactor", "refactor/issue-42"),
                ("fix", "fix/issue-42"),
                ("feat", "feat/issue-42"),
                ("chore", "chore/issue-42"),
                ("docs", "docs/issue-42"),
            ] {
                let mut ranked = issue(42, "タイトル🙂", body);
                ranked.score = 3.612_345;
                ranked.prefix = prefix.to_owned();
                let output = render_next_json(&repository, Some(&ranked));
                assert!(output.is_ok());
                let Ok(output) = output else { return };
                assert!(!output.chars().any(char::is_control));
                let expected = serde_json::json!({
                    "repository": "owner/repo",
                    "number": 42,
                    "title": "タイトル🙂",
                    "body": body,
                    "score": 3.612_345,
                    "readiness": "Yes",
                    "branch_name": branch_name,
                });
                assert!(matches!(serde_json::from_str::<serde_json::Value>(&output),
                    Ok(value) if value == expected));
            }
        }
        assert!(matches!(render_next_json(&repository, None), Ok(output) if output == "null"));
    }

    #[test]
    fn search_dialog_fits_five_content_rows_and_keeps_close_clickable() {
        for height in [7, 11, 24, 40] {
            let dialog = Dialog::search(100, height);
            assert!(dialog.is_ok());
            let Ok(dialog) = dialog else { return };
            let lines = dialog.lines(0);
            assert_eq!(lines.len(), 7);
            assert_eq!(dialog.rows, 1);
            assert_eq!(dialog.top, (height - 7) / 2 + 1);
            assert_eq!(dialog.button_y, dialog.top + 5);
            assert!(lines.get(1).is_some_and(|line| line.contains("Search")));
            for row in [2, 4] {
                assert_eq!(lines.get(row), Some(&format!("│{}│", " ".repeat(73))));
            }
            assert!(lines.get(5).is_some_and(|line| line.contains(CLOSE_BUTTON)));
            assert_eq!(dialog.button_at(dialog.left + 2, dialog.button_y), Some(0));
            assert_eq!(dialog.button_at(dialog.left + 2, dialog.top + 3), None);
        }
        assert!(Dialog::search(100, 6).is_err());
        assert!(Dialog::search(49, 24).is_err());
    }

    #[test]
    fn dialog_dimensions_fixed_buttons_and_unicode_content_stay_in_bounds() {
        for (width, height) in [(80, 24), (101, 31), (120, 40)] {
            let dialog = Dialog::new(
                width,
                height,
                "本文\x1b[31m",
                &"日本語🙂\x1b[31m\n".repeat(100),
                &[CLOSE_BUTTON, CREATE_BUTTON],
            );
            assert!(dialog.is_ok());
            let Ok(dialog) = dialog else {
                return;
            };
            assert_eq!(dialog.width, width * 3 / 4);
            assert_eq!(dialog.lines(0).len(), height - 4);
            for scroll in [0, dialog.max_scroll()] {
                let lines = dialog.lines(scroll);
                assert!(lines.iter().all(|line| {
                    line.replace("\x1b[7m", "").replace("\x1b[27m", "").width() == dialog.width
                }));
                assert!(!lines.join("").contains("\x1b[31m"));
                assert!(
                    lines
                        .get(dialog.button_y - dialog.top)
                        .is_some_and(|line| line.contains(CREATE_BUTTON))
                );
            }
            let close = dialog.left + 2;
            assert_eq!(dialog.button_at(close, dialog.button_y), Some(0));
            assert_eq!(
                dialog.button_at(close + CLOSE_BUTTON.len() - 1, dialog.button_y),
                Some(0)
            );
            assert_eq!(
                dialog.button_at(close + CLOSE_BUTTON.len(), dialog.button_y),
                None
            );
            assert_eq!(
                dialog.button_at(close + CLOSE_BUTTON.len() + 1, dialog.button_y),
                Some(1)
            );
            assert!(dialog.content_contains(dialog.left + 2, dialog.top + 3));
            assert!(!dialog.content_contains(dialog.left + 2, dialog.button_y));
        }
        assert!(Dialog::new(39, 24, "", "", &[CLOSE_BUTTON]).is_err());
        assert!(Dialog::new(80, 10, "", "", &[CLOSE_BUTTON]).is_err());
    }

    #[test]
    fn list_rows_use_natural_title_height_and_exclude_body_and_actions() {
        let title = "長いタイトル🙂".repeat(40);
        let table = render_issue_table(
            &[issue(42, &title, "本文だけの秘密"), issue(7, "next", "")],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(table.is_ok());
        let Ok(table) = table else {
            return;
        };
        assert!(table.issues.first().is_some_and(|row| row.lines.len() > 10));
        assert!(!table.text.contains("本文だけの秘密"));
        assert!(!table.text.contains("Branch:"));
        assert!(!table.text.contains("Create branch"));
        assert!(table.text.lines().all(|line| line.width() == 100));
    }

    fn issue(number: u64, title: &str, body: &str) -> RankedIssue {
        RankedIssue {
            issue: crate::model::Issue {
                details: Some(crate::model::test_issue_details()),
                number,
                title: title.to_owned(),
                body: body.to_owned(),
            },
            score: 3.6,
            prefix: "fix".to_owned(),
            readiness: crate::model::Readiness::Yes,
        }
    }

    #[test]
    fn wraps_unicode_newlines_and_renders_empty_table() {
        let issue = issue(42, "タイトル🙂", "本文\n長い日本語");
        for width in [80, 100, 120] {
            assert!(matches!(
                render_table(std::slice::from_ref(&issue), width),
                Ok(output) if output.lines().all(|line| line.width() == width)
            ));
        }
        assert!(matches!(
            render_table(std::slice::from_ref(&issue), 100),
            Ok(output) if output.contains("タイトル") && !output.contains("本文")
        ));
        assert!(matches!(render_table(&[], 100), Ok(output)
            if output.lines().count() == 4
                && output.contains("Ready")
                && output.lines().next().is_some_and(|line| line.trim() == "重要度: 0（低）〜4（高）")));
        assert!(render_table(&[], 10).is_err());
        assert!(render_table(std::slice::from_ref(&issue), 36).is_err());
    }

    #[test]
    fn displays_all_readiness_values_only_on_the_first_line() {
        use crate::model::Readiness;
        let issues = [
            Readiness::Yes,
            Readiness::NeedsInformation,
            Readiness::NeedsInvestigation,
        ]
        .map(|readiness| {
            let mut issue = issue(42, "タイトル🙂", "本文\n続き");
            issue.readiness = readiness;
            issue
        });
        for width in [95, 120] {
            let table = render_issue_table(
                &issues,
                width,
                &Ok(BTreeSet::from([42])),
                IssueSort::default(),
            );
            assert!(table.is_ok());
            let Ok(table) = table else { return };
            assert!(table.text.lines().all(|line| line.width() == width));
            for (ranked, region) in issues.iter().zip(&table.issues) {
                for (offset, line) in table
                    .text
                    .lines()
                    .skip(region.lines.start)
                    .take(region.lines.len())
                    .enumerate()
                {
                    assert_eq!(
                        line.split('│').nth(2).map(str::trim),
                        Some(if offset == 0 {
                            ranked.readiness.label()
                        } else {
                            ""
                        })
                    );
                }
            }
        }
        assert!(matches!(
            render_table(&issues, 50),
            Err(DisplayError::TerminalWidthTooSmall { width: 50 })
        ));
    }

    #[test]
    fn sanitizes_control_characters_before_width_calculation() {
        let unsafe_input = issue(42, "title\u{1b}[31m", "line\r\t");
        let safe_input = issue(42, "title [31m", "line  ");
        let unsafe_table = render_issue_table(
            &[unsafe_input],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        let safe_table = render_issue_table(
            &[safe_input],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(
            matches!((unsafe_table, safe_table), (Ok(unsafe_table), Ok(safe_table))
            if !unsafe_table.text.chars().any(|character| character.is_control() && character != '\n')
                && unsafe_table.text.lines().all(|line| line.width() == 100)
                && unsafe_table.text.contains("title [31m")
                && unsafe_table.text == safe_table.text)
        );
    }

    #[test]
    fn aligns_branch_states_and_keeps_errors_outside_issue_regions() {
        let issues = [
            issue(4, "first", "body"),
            issue(42, "next", ""),
            issue(420, "last", ""),
        ];
        for width in [80, 100, 120] {
            let table = render_issue_table(
                &issues,
                width,
                &Ok(BTreeSet::from([4, 420])),
                IssueSort::default(),
            );
            assert!(table.is_ok());
            let Ok(table) = table else { return };
            let numbers = table
                .issues
                .iter()
                .filter_map(|region| {
                    table
                        .text
                        .lines()
                        .nth(region.lines.start)?
                        .split('│')
                        .nth(3)
                })
                .collect::<Vec<_>>();
            assert_eq!(numbers, [" #4   🌿 ", " #42     ", " #420 🌿 "]);
            assert!(table.text.lines().all(|line| line.width() == width));
            assert_eq!(table.branch_errors, [] as [String; 0]);
            for (index, region) in table.issues.iter().enumerate() {
                let lines = table.lines(Some(index));
                assert!(
                    lines
                        .get(region.lines.start)
                        .is_some_and(|line| line.contains("\x1b[30;104m"))
                );
            }
            let unknown = render_issue_table(
                &issues,
                width,
                &Err("root missing\n\x1b[31m".into()),
                IssueSort::default(),
            );
            assert!(unknown.is_ok());
            let Ok(unknown) = unknown else { return };
            for region in &unknown.issues {
                assert!(
                    unknown
                        .text
                        .lines()
                        .nth(region.lines.start)
                        .is_some_and(|line| line
                            .split('│')
                            .nth(3)
                            .is_some_and(|cell| cell.contains('?')))
                );
            }
            assert!(
                unknown
                    .branch_errors
                    .concat()
                    .contains("ブランチ確認不能: root missing [31m")
            );
            assert!(
                unknown
                    .branch_errors
                    .iter()
                    .all(|line| line.width() <= width && !line.chars().any(char::is_control))
            );
        }
        let empty = render_issue_table(&[], 100, &Err("root missing".into()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert_eq!(
            empty.branch_errors.concat(),
            "ブランチ確認不能: root missing"
        );
    }

    #[test]
    fn highlights_issue_cells_and_preserves_borders() {
        let table = render_issue_table(
            &[
                issue(42, "タイトル│🙂", "本文\n\x1b[31m"),
                issue(43, "next", "body"),
            ],
            80,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(table.is_ok());
        let Ok(table) = table else { return };
        let plain = table.text.lines().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(table.lines(None), plain);
        for (selected, region) in table.issues.iter().enumerate() {
            let highlighted = table.lines(Some(selected));
            for (line_index, (line, original)) in highlighted.iter().zip(&plain).enumerate() {
                if region.lines.contains(&line_index) {
                    assert_eq!(line.matches("\x1b[30;104m").count(), 7);
                    assert_eq!(line.matches("\x1b[39;49m").count(), 7);
                    assert!(line.starts_with("│\x1b[30;104m "));
                    assert!(line.ends_with(" \x1b[39;49m│"));
                    assert_eq!(line.matches("\x1b[39;49m│").count(), 7);
                    let uncolored = line.replace("\x1b[30;104m", "").replace("\x1b[39;49m", "");
                    assert_eq!(&uncolored, original);
                    assert_eq!(uncolored.width(), 80);
                } else {
                    assert_eq!(line, original);
                }
            }
        }
        assert!(plain.iter().any(|line| line.contains("│🙂")));
        let empty = render_issue_table(&[], 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert!(empty.issues.is_empty());
        assert_eq!(empty.lines(None).join("\n"), empty.text);
    }
}
