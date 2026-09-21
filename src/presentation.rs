use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::RankedIssue;

#[derive(Debug)]
pub(crate) enum DisplayError {
    TerminalWidthUnavailable,
    TerminalWidthTooSmall { width: usize },
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
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
    if result != 0 || size.ws_col == 0 {
        return Err(DisplayError::TerminalWidthUnavailable);
    }
    Ok(usize::from(size.ws_col))
}

#[cfg(not(unix))]
pub(crate) fn terminal_width() -> Result<usize, DisplayError> {
    Err(DisplayError::TerminalWidthUnavailable)
}

fn score_label(score: f64) -> String {
    format!("{score}")
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in line.chars() {
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

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|line| wrap_line(line, width))
        .collect()
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

pub(crate) fn render_table(issues: &[RankedIssue], width: usize) -> Result<String, DisplayError> {
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
        return Ok(lines.join("\n"));
    }
    lines.push(horizontal_rule('├', '┼', '┤', widths));
    for (issue_index, ranked) in issues.iter().enumerate() {
        let content = if ranked.issue.body.is_empty() {
            ranked.issue.title.clone()
        } else {
            format!("{}\n{}", ranked.issue.title, ranked.issue.body)
        };
        for (line_index, content_line) in wrap_text(&content, widths[2]).iter().enumerate() {
            let importance = if line_index == 0 {
                score_label(ranked.score)
            } else {
                String::new()
            };
            let number = if line_index == 0 {
                format!("#{}", ranked.issue.number)
            } else {
                String::new()
            };
            lines.push(table_row([&importance, &number, content_line], widths));
        }
        if issue_index + 1 < issues.len() {
            lines.push(horizontal_rule('├', '┼', '┤', widths));
        }
    }
    lines.push(horizontal_rule('└', '┴', '┘', widths));
    debug_assert!(lines.iter().all(|line| line.width() == width));
    Ok(lines.join("\n"))
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
        }
    }

    #[test]
    fn wraps_unicode_newlines_and_renders_empty_table() {
        let issue = issue(42, "タイトル🙂", "本文\n長い日本語");
        for width in [26, 40, 60] {
            let output = render_table(std::slice::from_ref(&issue), width).unwrap();
            assert!(output.lines().all(|line| line.width() == width));
        }
        let output = render_table(std::slice::from_ref(&issue), 40).unwrap();
        assert!(output.contains("タイトル"));
        assert!(output.contains("本文"));
        let empty = render_table(&[], 40).unwrap();
        assert_eq!(empty.lines().count(), 3);
        assert!(render_table(&[], 10).is_err());
    }
}
