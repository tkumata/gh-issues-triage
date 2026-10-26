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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonRegion {
    pub(crate) issue_index: usize,
    pub(crate) x_start: usize,
    pub(crate) x_end: usize,
    pub(crate) line: usize,
}

pub(crate) struct SortRegion {
    pub(crate) column: SortColumn,
    pub(crate) x_start: usize,
    pub(crate) x_end: usize,
    pub(crate) line: usize,
}

pub(crate) const SEARCH_BUTTON: &str = "   Search   ";
pub(crate) const SEARCH_ROWS: usize = 2;

pub(crate) const HELP_BUTTON: &str = "   Help   ";
const CLOSE_BUTTON: &str = " Close ";

pub(crate) fn search_button_start(width: usize) -> usize {
    width.saturating_sub(SEARCH_BUTTON.len()) + 1
}

pub(crate) fn render_search_input(
    query: &str,
    focused: bool,
    width: usize,
) -> ([String; SEARCH_ROWS], usize) {
    let label = if focused { "Keywords> " } else { "Keywords: " };
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
    let (left, right) = if focused {
        ("\x1b[1m[\x1b[22m", "\x1b[1m]\x1b[22m")
    } else {
        ("[", "]")
    };
    let padding = " ".repeat(field_width.saturating_sub(content.width()));
    let content = if query.is_empty() {
        format!("\x1b[90m{content}\x1b[39m")
    } else {
        content.to_owned()
    };
    let text = format!("{label}{left} {content}{padding} {right} \x1b[7m{SEARCH_BUTTON}\x1b[27m");
    let cursor = label.width() + 3 + tail.width();
    ([text, pad_cell("", width)], cursor)
}

const HELP_TEXT: &str = "一覧画面の操作

- Tab キー: 一覧と検索入力を切り替え
- j/k・↑↓ キー: Issue を仮選択
- マウスオーバー: Issue を仮選択
- PageUp/PageDown キー: 一覧全体をスクロール
- マウスホイール: ポインター直下のセル内をスクロール
- 重要度・番号・Author・日付の見出しクリック: 並べ替え変更
  ↑: 昇順 / ↓: 降順
- Create branch のクリック: 対象 Issue のブランチを作成
  1 – 9/0: 表示順の先頭10件のブランチを作成
- 一覧画面で q / Ctrl+C: アプリの終了

検索方法

- Tab キーまたは入力欄のクリック: 検索入力へ移動
- Keywords>: 入力中 / Backspace: 末尾の文字を削除
- Enter キーまたは Search ボタンのクリックで検索を実行
  取得済み最大 100 件のタイトルと本文を検索
- 空白区切りの AND 検索。語順は不問
- ASCII 英字の大小は区別しない
- 空欄・空白だけの検索: 絞り込みを解除

表示の意味

- アイコンの意味
  - 🌿: ブランチあり
  - 空欄: ブランチなし
  - ?: 確認不能
- 重要度: 0 (低) ～ 4 (高)
- Ready: 仕様整理への着手可能度
  - Yes: 着手可能
  - Needs information: 情報の追加が必要
  - Needs investigation: 事前調査が必要";

pub(crate) struct HelpDialog {
    pub(crate) left: usize,
    pub(crate) top: usize,
    pub(crate) rows: usize,
    close_y: usize,
    width: usize,
    content: Vec<String>,
}

impl HelpDialog {
    pub(crate) fn new(width: usize, height: usize) -> Result<Self, DisplayError> {
        if width < 30 {
            return Err(DisplayError::TerminalWidthTooSmall { width });
        }
        if height < 7 {
            return Err(DisplayError::TerminalHeightTooSmall);
        }
        let dialog_width = width.min(88);
        let dialog_height = height.min(24);
        let top = (height - dialog_height) / 2 + 1;
        Ok(Self {
            left: (width - dialog_width) / 2 + 1,
            top,
            rows: dialog_height - 6,
            close_y: top + dialog_height - 2,
            width: dialog_width,
            content: HELP_TEXT
                .lines()
                .flat_map(|line| wrap_line(line, dialog_width - 4))
                .collect(),
        })
    }

    pub(crate) fn max_scroll(&self) -> usize {
        self.content.len().saturating_sub(self.rows)
    }

    pub(crate) fn closes_at(&self, x: usize, y: usize) -> bool {
        y == self.close_y && (self.left + 2..self.left + 2 + CLOSE_BUTTON.len()).contains(&x)
    }

    pub(crate) fn lines(&self, scroll: usize) -> Vec<String> {
        let mut lines = vec![format!("┌{}┐", "─".repeat(self.width - 2))];
        let heading = " Help: (マウスホイールでスクロール)";
        let heading = if heading.width() <= self.width - 2 {
            heading
        } else {
            " Help:"
        };
        lines.push(format!("│{}│", pad_cell(heading, self.width - 2)));
        lines.push(format!("│{}│", pad_cell("", self.width - 2)));
        for row in 0..self.rows {
            let text = self.content.get(scroll + row).map_or("", String::as_str);
            lines.push(format!("│ {} │", pad_cell(text, self.width - 4)));
        }
        lines.push(format!("│{}│", pad_cell("", self.width - 2)));
        lines.push(format!(
            "│ \x1b[7m{CLOSE_BUTTON}\x1b[27m{}│",
            pad_cell(" Esc/Enter", self.width - 3 - CLOSE_BUTTON.len())
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
    let mut table = render_table_with_buttons(issues, width, branches, sort)?;
    if issues.is_empty() {
        table.text.push('\n');
        table.text.push_str(&pad_cell("No matching issues.", width));
    }
    Ok(table)
}

pub(crate) struct RenderedTable {
    pub(crate) text: String,
    pub(crate) buttons: Vec<ButtonRegion>,
    pub(crate) issues: Vec<IssueRegion>,
    pub(crate) branch_errors: Vec<String>,
    pub(crate) sort_headers: Vec<SortRegion>,
    widths: [usize; 7],
}

pub(crate) struct IssueRegion {
    pub(crate) lines: std::ops::Range<usize>,
    highlighted: Vec<String>,
    cells: [Vec<String>; 7],
    button_lines: std::ops::Range<usize>,
    offsets: [usize; 7],
}

impl RenderedTable {
    pub(crate) fn limit_rows(&mut self, budget: usize) -> Result<(), DisplayError> {
        let Some(first) = self.issues.first() else {
            return Ok(());
        };
        if budget < 3 {
            return Err(DisplayError::TerminalHeightTooSmall);
        }
        let mut lines = self
            .text
            .lines()
            .take(first.lines.start)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        self.buttons.clear();
        let count = self.issues.len();
        for (index, issue) in self.issues.iter_mut().enumerate() {
            let limit = budget / 3 + usize::from(index < budget % 3);
            let (rows, buttons) = issue.render(self.widths, index, lines.len(), limit);
            lines.extend(rows);
            self.buttons.extend(buttons);
            let last = index + 1 == count;
            lines.push(horizontal_rule(
                if last { '└' } else { '├' },
                if last { '┴' } else { '┼' },
                if last { '┘' } else { '┤' },
                self.widths,
            ));
        }
        self.text = lines.join("\n");
        Ok(())
    }

    pub(crate) fn scroll_cell(&mut self, x: usize, line: usize, down: bool) {
        let Some(issue) = self
            .issues
            .iter_mut()
            .find(|issue| issue.lines.contains(&line))
        else {
            return;
        };
        let mut start = 2;
        for ((cell, offset), width) in issue.cells.iter().zip(&mut issue.offsets).zip(self.widths) {
            if (start..start + width + 2).contains(&x) {
                *offset = if down {
                    offset
                        .saturating_add(1)
                        .min(cell.len().saturating_sub(issue.lines.len()))
                } else {
                    offset.saturating_sub(1)
                };
                return;
            }
            start += width + 3;
        }
    }

    pub(crate) fn preserve_cell_offsets(&mut self, previous: &Self) {
        for (issue, previous) in self.issues.iter_mut().zip(&previous.issues) {
            issue.offsets = previous.offsets;
        }
    }

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
    render_table_with_buttons(issues, width, &Ok(BTreeSet::new()), IssueSort::default())
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

fn issue_content(ranked: &RankedIssue, width: usize, issue_index: usize) -> Vec<(String, bool)> {
    let mut content = ranked
        .issue
        .title
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !ranked.issue.body.is_empty() {
        content.extend(ranked.issue.body.split('\n').map(str::to_owned));
    }
    content.push(String::new());
    content.push(format!("Branch: {}", crate::branch::branch_name(ranked)));
    let button_index = content.len();
    content.push(format!("[{}] Create branch", issue_index + 1));
    let mut content_lines = Vec::new();
    for (content_index, content_line) in content.into_iter().enumerate() {
        content_lines.extend(
            wrap_line(&content_line, width)
                .into_iter()
                .map(|line| (line, content_index == button_index)),
        );
    }
    content_lines
}

fn render_issue(
    ranked: &RankedIssue,
    details: &crate::model::IssueDetails,
    widths: [usize; 7],
    branches: &Result<BTreeSet<u64>, String>,
    issue_index: usize,
    first_line: usize,
) -> (Vec<String>, IssueRegion, Vec<ButtonRegion>) {
    let number_label_width = widths[2] - 1 - "🌿".width();
    let content = issue_content(ranked, widths[6], issue_index);
    let button_start = content
        .iter()
        .position(|(_, button)| *button)
        .unwrap_or(content.len());
    let button_end = content.len();
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
            number_label_width,
            branches,
        )],
        wrap_line(details.author.as_deref().unwrap_or("-"), widths[3]),
        wrap_line(&assignees, widths[4]),
        vec![details.created_at.jst_date.clone()],
        content.into_iter().map(|(line, _)| line).collect(),
    ];
    let mut region = IssueRegion {
        lines: first_line..first_line,
        highlighted: Vec::new(),
        cells,
        button_lines: button_start..button_end,
        offsets: [0; 7],
    };
    let (rows, buttons) = region.render(widths, issue_index, first_line, usize::MAX);
    (rows, region, buttons)
}

impl IssueRegion {
    fn render(
        &mut self,
        widths: [usize; 7],
        issue_index: usize,
        first_line: usize,
        limit: usize,
    ) -> (Vec<String>, Vec<ButtonRegion>) {
        let height = self
            .cells
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .min(limit);
        for (offset, cell) in self.offsets.iter_mut().zip(&self.cells) {
            *offset = (*offset).min(cell.len().saturating_sub(height));
        }
        let mut rows = Vec::new();
        let mut buttons = Vec::new();
        self.highlighted.clear();
        for row in 0..height {
            let cells = std::array::from_fn(|column| {
                self.cells
                    .get(column)
                    .zip(self.offsets.get(column))
                    .and_then(|(cell, offset)| cell.get(offset + row))
                    .map_or("", String::as_str)
            });
            rows.push(table_row(cells, widths, false));
            self.highlighted.push(table_row(cells, widths, true));
            if self.button_lines.contains(&(self.offsets[6] + row)) {
                let x_start = widths.iter().take(6).sum::<usize>() + 21;
                buttons.push(ButtonRegion {
                    issue_index,
                    x_start,
                    x_end: x_start + cells[6].width() - 1,
                    line: first_line + row,
                });
            }
        }
        self.lines = first_line..first_line + height;
        (rows, buttons)
    }
}

pub(crate) fn render_table_with_buttons(
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
            buttons: Vec::new(),
            issues: Vec::new(),
            branch_errors: branch_errors(branches, width),
            sort_headers,
            widths,
        });
    }
    lines.push(horizontal_rule('├', '┼', '┤', widths));
    let mut buttons = Vec::new();
    let mut issue_regions = Vec::new();
    for (issue_index, (ranked, details)) in issues.iter().zip(details).enumerate() {
        let (rows, region, issue_buttons) =
            render_issue(ranked, details, widths, branches, issue_index, lines.len());
        lines.extend(rows);
        issue_regions.push(region);
        buttons.extend(issue_buttons);
        if issue_index + 1 < issues.len() {
            lines.push(horizontal_rule('├', '┼', '┤', widths));
        }
    }
    lines.push(horizontal_rule('└', '┴', '┘', widths));
    debug_assert!(lines.iter().all(|line| line.width() == width));
    Ok(RenderedTable {
        text: lines.join("\n"),
        buttons,
        issues: issue_regions,
        branch_errors: branch_errors(branches, width),
        sort_headers,
        widths,
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
    fn caps_rows_distributes_remainders_and_rejects_insufficient_height() {
        let issues = [1, 2, 3, 4].map(|number| issue(number, "title", &"body\n".repeat(30)));
        let table =
            render_table_with_buttons(&issues, 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(table.is_ok());
        let Ok(mut table) = table else { return };
        for (budget, expected) in [(9, [3, 3, 3, 3]), (10, [4, 3, 3, 3]), (11, [4, 4, 3, 3])] {
            assert!(table.limit_rows(budget).is_ok());
            assert_eq!(
                table
                    .issues
                    .iter()
                    .map(|row| row.lines.len())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(table.text.lines().all(|line| line.width() == 100));
            assert_eq!(table.buttons, [] as [ButtonRegion; 0]);
        }
        assert!(matches!(
            table.limit_rows(2),
            Err(DisplayError::TerminalHeightTooSmall)
        ));
        assert!(table.limit_rows(3).is_ok());
        assert!(table.issues.iter().all(|row| row.lines.len() == 1));
        let short = render_table_with_buttons(
            &[issue(1, "title", "")],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(short.is_ok());
        let Ok(mut short) = short else { return };
        let original = short.text.clone();
        assert!(short.limit_rows(300).is_ok());
        assert_eq!(short.text, original);
    }

    #[test]
    fn scrolls_cells_independently_and_exposes_only_visible_button_fragments() {
        let mut ranked = issue(42, "日本語🙂", &"本文\n".repeat(30));
        if let Some(details) = &mut ranked.issue.details {
            details.author = Some("a".repeat(100));
            details.assignees = vec!["b".repeat(100)];
        }
        let table = render_table_with_buttons(
            &[ranked.clone()],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(table.is_ok());
        let Ok(mut table) = table else { return };
        assert!(table.limit_rows(9).is_ok());
        let Some(first) = table.issues.first() else {
            return;
        };
        let line = first.lines.start;
        let author_x = table.widths.iter().take(3).sum::<usize>() + 11;
        let content_x = table.widths.iter().take(6).sum::<usize>() + 21;
        let original = table.text.clone();
        for x in [0, 1, author_x - 2, 101] {
            table.scroll_cell(x, line, true);
        }
        table.scroll_cell(content_x, line - 1, true);
        assert!(table.limit_rows(9).is_ok());
        assert_eq!(table.text, original);
        table.scroll_cell(author_x, line, true);
        table.scroll_cell(content_x, line, true);
        assert!(table.limit_rows(9).is_ok());
        assert!(
            table
                .issues
                .first()
                .is_some_and(|row| row.offsets == [0, 0, 0, 1, 0, 0, 1])
        );
        assert!(!table.text.contains("日本語"));
        assert_eq!(table.buttons, [] as [ButtonRegion; 0]);
        for _ in 0..200 {
            table.scroll_cell(content_x, line, true);
        }
        assert!(table.limit_rows(9).is_ok());
        assert_ne!(table.buttons, [] as [ButtonRegion; 0]);
        for button in &table.buttons {
            assert_eq!(button.issue_index, 0);
            assert!(
                table
                    .text
                    .lines()
                    .nth(button.line)
                    .is_some_and(|row| row.contains("Create branch"))
            );
        }
        let bottom = table.text.clone();
        table.scroll_cell(content_x, line, true);
        assert!(table.limit_rows(9).is_ok());
        assert_eq!(table.text, bottom);
        let refreshed = render_table_with_buttons(
            &[ranked],
            100,
            &Ok(BTreeSet::from([42])),
            IssueSort::default(),
        );
        assert!(refreshed.is_ok());
        let Ok(mut refreshed) = refreshed else { return };
        refreshed.preserve_cell_offsets(&table);
        assert!(refreshed.limit_rows(9).is_ok());
        assert!(refreshed.text.contains("🌿"));
        assert_ne!(refreshed.buttons, [] as [ButtonRegion; 0]);
        for _ in 0..200 {
            table.scroll_cell(content_x, line, false);
        }
        assert!(table.limit_rows(9).is_ok());
        assert!(table.text.contains("日本語🙂"));
        assert_eq!(table.buttons, [] as [ButtonRegion; 0]);
        assert!(table.text.lines().all(|row| row.width() == 100));
    }

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
        let table = render_table_with_buttons(&[ranked], 100, &Ok(BTreeSet::new()), sort);
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
        let unassigned = render_table_with_buttons(
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
    fn search_input_stays_one_line_and_shows_the_unicode_tail_and_cursor() {
        for width in [34, 37, 40, 60] {
            for focused in [false, true] {
                let query = format!("{}末尾🙂", "長い検索語".repeat(20));
                let (lines, cursor) = render_search_input(&query, focused, width);
                let label = if focused { "Keywords> " } else { "Keywords: " };
                assert!(lines.first().is_some_and(|line| line.starts_with(label)));
                assert_eq!(lines.len(), 2);
                assert_eq!(lines[1], " ".repeat(width));
                assert!(lines[0].ends_with(&format!(" \x1b[7m{SEARCH_BUTTON}\x1b[27m")));
                assert!(lines.iter().all(|line| {
                    let visible = line
                        .replace("\x1b[7m", "")
                        .replace("\x1b[27m", "")
                        .replace("\x1b[1m", "")
                        .replace("\x1b[22m", "");
                    visible.width() == width && !visible.contains(['\n', '\x1b'])
                }));
                let input = lines[0]
                    .split(" \x1b[7m")
                    .next()
                    .unwrap_or("")
                    .replace("\x1b[1m", "")
                    .replace("\x1b[22m", "");
                assert!(input.starts_with(&format!("{label}[ ")));
                assert!(input.ends_with(" ]"));
                let before_cursor = input.trim_end_matches(']').trim_end();
                assert!(before_cursor.ends_with("末尾🙂"));
                assert!(cursor <= width - 2);
                assert_eq!(cursor, before_cursor.width() + 1);
            }
        }
        let empty = render_search_results(&[], 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(
            matches!(empty, Ok(table) if table.text.contains("No matching issues.")
            && table.buttons.is_empty() && table.issues.is_empty()
            && table.text.lines().all(|line| line.width() == 100))
        );
    }

    #[test]
    fn search_field_shows_a_placeholder_only_when_empty_and_highlights_its_brackets() {
        for focused in [false, true] {
            for query in ["", "日本🙂", " "] {
                let (lines, cursor) = render_search_input(query, focused, 60);
                let input = &lines[0];
                let visible = input
                    .replace("\x1b[7m", "")
                    .replace("\x1b[27m", "")
                    .replace("\x1b[1m", "")
                    .replace("\x1b[22m", "")
                    .replace("\x1b[90m", "")
                    .replace("\x1b[39m", "");
                assert_eq!(visible.width(), 60);
                assert!(visible.ends_with(&format!(" ] {SEARCH_BUTTON}")));
                assert_eq!(visible.contains("Type keywords..."), query.is_empty());
                if query.is_empty() {
                    assert!(input.contains("\x1b[90mType keywords...\x1b[39m "));
                } else {
                    assert!(!input.contains("\x1b[90m"));
                    assert!(!input.contains("\x1b[39m"));
                }
                assert_eq!(cursor, 13 + query.width());
                if focused {
                    assert!(input.starts_with("Keywords> \x1b[1m[\x1b[22m "));
                    assert!(input.contains(" \x1b[1m]\x1b[22m \x1b[7m"));
                } else {
                    assert!(input.starts_with("Keywords: [ "));
                    assert!(!visible.contains('\x1b'));
                }
                if !query.is_empty() {
                    assert!(visible[12..].starts_with(query));
                }
            }
        }
        let (lines, cursor) = render_search_input("", true, 33);
        assert!(lines[0].contains(" \x1b[90mType \x1b[39m "));
        assert_eq!(cursor, 13);
    }

    #[test]
    fn help_dialog_fits_the_terminal_and_keeps_close_button_padding_clickable() {
        for (width, height) in [(30, 7), (39, 24), (40, 24), (80, 24), (120, 40)] {
            let result = HelpDialog::new(width, height);
            assert!(result.is_ok());
            let Ok(dialog) = result else { return };
            for scroll in [0, dialog.max_scroll()] {
                let lines = dialog.lines(scroll);
                assert!(dialog.top + lines.len() - 1 <= height);
                let blank = format!("│{}│", " ".repeat(dialog.width - 2));
                assert_eq!(lines.get(2), Some(&blank));
                assert_eq!(lines.get(lines.len() - 3), Some(&blank));
                assert!(
                    lines
                        .get(dialog.close_y - dialog.top)
                        .is_some_and(|line| line.contains(CLOSE_BUTTON))
                );
                assert!(lines.iter().all(|line| {
                    let visible = line.replace("\x1b[7m", "").replace("\x1b[27m", "");
                    visible.width() == dialog.width && dialog.left + visible.width() - 1 <= width
                }));
            }
            let start = dialog.left + 2;
            let end = start + CLOSE_BUTTON.len() - 1;
            assert!(dialog.closes_at(start, dialog.close_y));
            assert!(dialog.closes_at(end, dialog.close_y));
            assert!(!dialog.closes_at(start - 1, dialog.close_y));
            assert!(!dialog.closes_at(end + 1, dialog.close_y));
            assert!(!dialog.closes_at(start, dialog.close_y - 1));
            assert!(dialog.max_scroll() > 0);
        }
        assert!(HelpDialog::new(29, 24).is_err());
        assert!(HelpDialog::new(80, 6).is_err());
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
            Ok(output) if output.contains("タイトル") && output.contains("本文")
        ));
        assert!(matches!(render_table(&[], 100), Ok(output)
            if output.lines().count() == 4
                && output.contains("Ready")
                && output.lines().next().is_some_and(|line| line.trim() == "重要度: 0（低）〜4（高）")));
        assert!(render_table(&[], 10).is_err());
        assert!(render_table(std::slice::from_ref(&issue), 36).is_err());
    }

    #[test]
    fn places_button_region_after_each_issue_body_and_within_table_width() {
        let issues = [issue(42, "title", "first\nsecond"), issue(43, "next", "")];
        let rendered =
            render_table_with_buttons(&issues, 110, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(matches!(rendered, Ok(table)
            if table.buttons.len() == 2
                && table.buttons.first().is_some_and(|button| button.issue_index == 0)
                && table.buttons.get(1).is_some_and(|button| button.issue_index == 1)
                && table.text.lines().all(|line| line.width() == 110)
                && table.buttons.first().is_some_and(|button| button.x_start == 66 && button.x_end - button.x_start + 1 == "[1] Create branch".width())
                && table.buttons.first().and_then(|button| table.text.lines().nth(button.line - 1)).is_some_and(|line| line.contains("Branch:"))
                && table.buttons.first().and_then(|button| table.text.lines().nth(button.line - 2)).is_some_and(|line| line.split('│').nth(7).is_some_and(|cell| cell.trim().is_empty()))
                && table.buttons.first().and_then(|button| table.text.lines().nth(button.line)).is_some_and(|line| line.contains("[1]"))
                && table.buttons.get(1).and_then(|button| table.text.lines().nth(button.line)).is_some_and(|line| line.contains("[2]"))));
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
            let table = render_table_with_buttons(
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
            for button in &table.buttons {
                assert_eq!(button.x_start, 80);
                let cells = table
                    .text
                    .lines()
                    .nth(button.line)
                    .and_then(|line| line.split('│').nth(7));
                assert_eq!(
                    cells.map(|cell| cell.trim_end().width()),
                    Some(button.x_end - button.x_start + 2)
                );
            }
        }
        assert!(matches!(
            render_table(&issues, 50),
            Err(DisplayError::TerminalWidthTooSmall { width: 50 })
        ));
    }

    #[test]
    fn records_every_wrapped_button_fragment_as_a_clickable_region() {
        let rendered = render_table_with_buttons(
            &[issue(42, "title", "body")],
            75,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(matches!(rendered, Ok(table)
            if table.buttons.len() > 1
                && table.buttons.iter().all(|button| button.issue_index == 0 && button.x_end >= button.x_start)
                && table.buttons.iter().map(|button| button.x_end - button.x_start + 1).sum::<usize>() == "[1] Create branch".width()
                && table.buttons.iter().all(|button| table.text.lines().nth(button.line).is_some_and(|line| line.chars().any(|character| character != '│' && character != ' ')))));
    }

    #[test]
    fn sanitizes_control_characters_before_width_and_button_calculation() {
        let unsafe_input = issue(42, "title\u{1b}[31m", "line\r\t");
        let safe_input = issue(42, "title [31m", "line  ");
        let unsafe_table = render_table_with_buttons(
            &[unsafe_input],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        let safe_table = render_table_with_buttons(
            &[safe_input],
            100,
            &Ok(BTreeSet::new()),
            IssueSort::default(),
        );
        assert!(
            matches!((unsafe_table, safe_table), (Ok(unsafe_table), Ok(safe_table))
            if !unsafe_table.text.chars().any(|character| character.is_control() && character != '\n')
                && unsafe_table.text.lines().all(|line| line.width() == 100)
                && unsafe_table.buttons == safe_table.buttons
                && unsafe_table.text.contains("title [31m")
                && unsafe_table.text.contains("line  "))
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
            let table = render_table_with_buttons(
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
            let unknown = render_table_with_buttons(
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
            assert_eq!(unknown.buttons, table.buttons);
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
        let empty =
            render_table_with_buttons(&[], 100, &Err("root missing".into()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert_eq!(
            empty.branch_errors.concat(),
            "ブランチ確認不能: root missing"
        );
    }

    #[test]
    fn highlights_issue_cells_and_preserves_borders() {
        let table = render_table_with_buttons(
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
        let empty = render_table_with_buttons(&[], 100, &Ok(BTreeSet::new()), IssueSort::default());
        assert!(empty.is_ok());
        let Ok(empty) = empty else { return };
        assert!(empty.issues.is_empty());
        assert_eq!(empty.lines(None).join("\n"), empty.text);
    }
}
