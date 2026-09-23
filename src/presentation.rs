use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::RankedIssue;

#[derive(Debug)]
pub(crate) enum DisplayError {
    TerminalWidthUnavailable,
    TerminalWidthTooSmall { width: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonRegion {
    pub(crate) issue_index: usize,
    pub(crate) x_start: usize,
    pub(crate) x_end: usize,
    pub(crate) line: usize,
}

pub(crate) struct RenderedTable {
    pub(crate) text: String,
    pub(crate) buttons: Vec<ButtonRegion>,
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TerminalWidthUnavailable => formatter.write_str("terminal width is unavailable"),
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

fn score_label(score: f64) -> String {
    format!("{score}")
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

fn horizontal_rule(left: char, junction: char, right: char, widths: [usize; 3]) -> String {
    format!(
        "{left}{}{}{}{}{}{right}",
        "─".repeat(widths[0] + 2),
        junction,
        "─".repeat(widths[1] + 2),
        junction,
        "─".repeat(widths[2] + 2)
    )
}

fn table_row(cells: [&str; 3], widths: [usize; 3]) -> String {
    format!(
        "│ {} │ {} │ {} │",
        pad_cell(cells[0], widths[0]),
        pad_cell(cells[1], widths[1]),
        pad_cell(cells[2], widths[2])
    )
}

#[cfg(test)]
fn render_table(issues: &[RankedIssue], width: usize) -> Result<String, DisplayError> {
    render_table_with_buttons(issues, width).map(|table| table.text)
}

pub(crate) fn render_table_with_buttons(
    issues: &[RankedIssue],
    width: usize,
) -> Result<RenderedTable, DisplayError> {
    let importance_width = issues
        .iter()
        .map(|issue| score_label(issue.score).width())
        .max()
        .unwrap_or(0)
        .max("重要度".width());
    let number_width = issues
        .iter()
        .map(|issue| format!("#{}", issue.issue.number).width())
        .max()
        .unwrap_or(0)
        .max("番号".width());
    let fixed_width = importance_width + number_width + 10;
    if width <= fixed_width || width - fixed_width < "Issues".width() {
        return Err(DisplayError::TerminalWidthTooSmall { width });
    }
    let widths = [importance_width, number_width, width - fixed_width];
    let mut lines = vec![horizontal_rule('┌', '┬', '┐', widths)];
    lines.push(table_row(["重要度", "番号", "Issues"], widths));
    if issues.is_empty() {
        lines.push(horizontal_rule('└', '┴', '┘', widths));
        return Ok(RenderedTable {
            text: lines.join("\n"),
            buttons: Vec::new(),
        });
    }
    lines.push(horizontal_rule('├', '┼', '┤', widths));
    let content_column = widths[0] + widths[1] + 9;
    let mut buttons = Vec::new();
    for (issue_index, ranked) in issues.iter().enumerate() {
        let mut content = ranked
            .issue
            .title
            .split('\n')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !ranked.issue.body.is_empty() {
            content.extend(ranked.issue.body.split('\n').map(str::to_owned));
        }
        content.push(format!("Branch: {}", crate::branch::branch_name(ranked)));
        let button_index = content.len();
        content.push(format!("[{}] Create branch", issue_index + 1));
        let mut issue_line_index = 0;
        for (content_index, content_line) in content.into_iter().enumerate() {
            let wrapped_lines = wrap_line(&content_line, widths[2]);
            for wrapped in &wrapped_lines {
                let importance = if issue_line_index == 0 {
                    score_label(ranked.score)
                } else {
                    String::new()
                };
                let number = if issue_line_index == 0 {
                    format!("#{}", ranked.issue.number)
                } else {
                    String::new()
                };
                lines.push(table_row([&importance, &number, wrapped], widths));
                if content_index == button_index {
                    buttons.push(ButtonRegion {
                        issue_index,
                        x_start: content_column,
                        x_end: content_column + wrapped.width() - 1,
                        line: lines.len() - 1,
                    });
                }
                issue_line_index += 1;
            }
        }
        if issue_index + 1 < issues.len() {
            lines.push(horizontal_rule('├', '┼', '┤', widths));
        }
    }
    lines.push(horizontal_rule('└', '┴', '┘', widths));
    debug_assert!(lines.iter().all(|line| line.width() == width));
    Ok(RenderedTable {
        text: lines.join("\n"),
        buttons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, title: &str, body: &str) -> RankedIssue {
        RankedIssue {
            issue: crate::model::Issue {
                number,
                title: title.to_owned(),
                body: body.to_owned(),
                source_order: 0,
            },
            score: 3.6,
            prefix: "fix".to_owned(),
        }
    }

    #[test]
    fn wraps_unicode_newlines_and_renders_empty_table() {
        let issue = issue(42, "タイトル🙂", "本文\n長い日本語");
        for width in [26, 40, 60] {
            assert!(matches!(
                render_table(std::slice::from_ref(&issue), width),
                Ok(output) if output.lines().all(|line| line.width() == width)
            ));
        }
        assert!(matches!(
            render_table(std::slice::from_ref(&issue), 40),
            Ok(output) if output.contains("タイトル") && output.contains("本文")
        ));
        assert!(matches!(render_table(&[], 40), Ok(output) if output.lines().count() == 3));
        assert!(render_table(&[], 10).is_err());
    }

    #[test]
    fn places_button_region_after_each_issue_body_and_within_table_width() {
        let issues = [issue(42, "title", "first\nsecond"), issue(43, "next", "")];
        let rendered = render_table_with_buttons(&issues, 40);
        assert!(matches!(rendered, Ok(table)
            if table.buttons.len() == 2
                && table.buttons.first().is_some_and(|button| button.issue_index == 0)
                && table.buttons.get(1).is_some_and(|button| button.issue_index == 1)
                && table.text.lines().all(|line| line.width() == 40)
                && table.buttons.first().is_some_and(|button| button.x_start == 19 && button.x_end - button.x_start + 1 == "[1] Create branch".width())
                && table.buttons.first().and_then(|button| table.text.lines().nth(button.line)).is_some_and(|line| line.contains("[1]"))
                && table.buttons.get(1).and_then(|button| table.text.lines().nth(button.line)).is_some_and(|line| line.contains("[2]"))));
    }

    #[test]
    fn records_every_wrapped_button_fragment_as_a_clickable_region() {
        let rendered = render_table_with_buttons(&[issue(42, "title", "body")], 26);
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
        let unsafe_table = render_table_with_buttons(&[unsafe_input], 40);
        let safe_table = render_table_with_buttons(&[safe_input], 40);
        assert!(
            matches!((unsafe_table, safe_table), (Ok(unsafe_table), Ok(safe_table))
            if !unsafe_table.text.chars().any(|character| character.is_control() && character != '\n')
                && unsafe_table.text.lines().all(|line| line.width() == 40)
                && unsafe_table.buttons == safe_table.buttons
                && unsafe_table.text.contains("title [31m")
                && unsafe_table.text.contains("line  "))
        );
    }
}
