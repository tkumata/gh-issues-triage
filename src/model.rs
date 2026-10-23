use std::cmp::Ordering;

pub(crate) const SCORE_CRITERIA: [&str; 5] = [
    "対応不要、情報提供のみ、または影響が確認できない",
    "影響が小さく、容易な回避策がある",
    "通常機能に影響するが、業務を止めない",
    "主要機能を妨げる、広い利用者に影響する、または回避が困難",
    "データ損失、セキュリティ、サービス停止など直ちに対応すべき影響",
];

pub(crate) const BRANCH_CRITERIA: [(&str, &str); 5] = [
    (
        "refactor",
        "internal improvement without a user-facing behavior change",
    ),
    ("fix", "fix an existing defect"),
    ("feat", "add or extend user-facing functionality"),
    ("chore", "maintenance, dependencies, or development tooling"),
    ("docs", "documentation only"),
];

pub(crate) const READINESS_CRITERIA: [(&str, &str); 3] = [
    (
        "Yes",
        "A concrete desired behavior or concrete observed problem is described, so an issue-specific requirements outline can be written without inventing the central behavior or domain rule. Details of that already identified behavior may remain Open Questions, including exact fields, completion criteria, reproduction steps, cause, implementation approach, and feasibility.",
    ),
    (
        "Needs information",
        "Only a topic or generic wish is given, or an essential behavior or domain rule defining what the requested feature does is missing. Clarification is needed to avoid inventing the requirements. For a checker, merely naming dependencies does not say what should be detected or considered a violation. A list of questions or generic research plan alone is insufficient. Prefer this when information and investigation both block drafting.",
    ),
    (
        "Needs investigation",
        "The concrete intended behavior or problem is known, but factual investigation is indispensable before specification drafting can begin. Ordinary code inspection, identifying a bug cause, or research recordable during drafting does not qualify.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readiness {
    Yes,
    NeedsInformation,
    NeedsInvestigation,
}

impl Readiness {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Yes => "Yes",
            Self::NeedsInformation => "Needs information",
            Self::NeedsInvestigation => "Needs investigation",
        }
    }

    pub(crate) fn from_choice(choice: &str) -> Option<Self> {
        match choice {
            "Yes" => Some(Self::Yes),
            "Needs information" => Some(Self::NeedsInformation),
            "Needs investigation" => Some(Self::NeedsInvestigation),
            _ => None,
        }
    }
}

pub(crate) fn issue_branch_number(reference: &str) -> Option<u64> {
    let (prefix, suffix) = reference
        .strip_prefix("refs/heads/")?
        .split_once("/issue-")?;
    if !BRANCH_CRITERIA.iter().any(|(key, _)| *key == prefix) {
        return None;
    }
    let number = suffix.parse::<u64>().ok()?;
    (suffix == number.to_string()).then_some(number)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryRef {
    pub(crate) owner: String,
    pub(crate) repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) details: Option<IssueDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueDetails {
    pub(crate) author: Option<String>,
    pub(crate) assignees: Vec<String>,
    pub(crate) created_at: IssueDate,
    pub(crate) fetched_order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueDate {
    utc: [u32; 6],
    pub(crate) jst_date: String,
}

impl IssueDate {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let parts = value.split(['-', 'T', ':', 'Z']).collect::<Vec<_>>();
        let [year, month, day, hour, minute, second, ""] = parts.as_slice() else {
            return None;
        };
        let utc = [year, month, day, hour, minute, second].map(|part| part.parse::<u32>().ok());
        let [
            Some(year),
            Some(month),
            Some(day),
            Some(hour),
            Some(minute),
            Some(second),
        ] = utc
        else {
            return None;
        };
        if format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z") != value
            || !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let (mut jst_year, mut jst_month, mut jst_day) = (year, month, day);
        if hour >= 15 {
            jst_day += 1;
            if jst_day > days_in_month(jst_year, jst_month) {
                jst_day = 1;
                jst_month += 1;
                if jst_month > 12 {
                    jst_month = 1;
                    jst_year += 1;
                }
            }
        }
        (jst_year <= 9999).then(|| Self {
            utc: [year, month, day, hour, minute, second],
            jst_date: format!("{jst_year:04}-{jst_month:02}-{jst_day:02}"),
        })
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortColumn {
    Importance,
    Number,
    Author,
    CreatedAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IssueSort {
    pub(crate) column: SortColumn,
    pub(crate) descending: bool,
}

impl Default for IssueSort {
    fn default() -> Self {
        Self {
            column: SortColumn::Importance,
            descending: true,
        }
    }
}

impl IssueSort {
    pub(crate) fn toggle(&mut self, column: SortColumn) {
        self.descending = self.column == column && !self.descending;
        self.column = column;
    }

    pub(crate) fn apply(self, issues: &mut [RankedIssue]) {
        issues.sort_by(|left, right| {
            let left_details = left.issue.details.as_ref();
            let right_details = right.issue.details.as_ref();
            let order = match self.column {
                SortColumn::Importance => left.score.total_cmp(&right.score),
                SortColumn::Number => left.issue.number.cmp(&right.issue.number),
                SortColumn::Author => {
                    let left = left_details.and_then(|details| details.author.as_deref());
                    let right = right_details.and_then(|details| details.author.as_deref());
                    match (left, right) {
                        (Some(left), Some(right)) => left.cmp(right),
                        (None, Some(_)) => return Ordering::Greater,
                        (Some(_), None) => return Ordering::Less,
                        (None, None) => Ordering::Equal,
                    }
                }
                SortColumn::CreatedAt => left_details
                    .map(|details| details.created_at.utc)
                    .cmp(&right_details.map(|details| details.created_at.utc)),
            };
            let order = if self.descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(|| {
                left_details
                    .map(|details| details.fetched_order)
                    .cmp(&right_details.map(|details| details.fetched_order))
            })
        });
    }
}

#[cfg(test)]
pub(crate) fn test_issue_details() -> IssueDetails {
    IssueDetails {
        author: Some("octocat".into()),
        assignees: vec![],
        created_at: IssueDate {
            utc: [2026, 10, 23, 0, 0, 0],
            jst_date: "2026-10-23".into(),
        },
        fetched_order: 0,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RankedIssue {
    pub(crate) issue: Issue,
    pub(crate) score: f64,
    pub(crate) prefix: String,
    pub(crate) readiness: Readiness,
}

pub(crate) fn search_issues(issues: &[RankedIssue], query: &str) -> Vec<RankedIssue> {
    let query = query.to_ascii_lowercase();
    issues
        .iter()
        .filter(|ranked| {
            let title = ranked.issue.title.to_ascii_lowercase();
            let body = ranked.issue.body.to_ascii_lowercase();
            query
                .split_whitespace()
                .all(|word| title.contains(word) || body.contains(word))
        })
        .cloned()
        .collect()
}

pub(crate) fn next_ready_issue(ranked: &[RankedIssue]) -> Option<&RankedIssue> {
    ranked
        .iter()
        .find(|issue| issue.readiness == Readiness::Yes)
}

pub(crate) fn rank_issues(
    issues: Vec<Issue>,
    scores: Vec<(f64, String, Readiness)>,
) -> Vec<RankedIssue> {
    let mut ranked = issues
        .into_iter()
        .zip(scores)
        .map(|(issue, (score, prefix, readiness))| RankedIssue {
            issue,
            score,
            prefix,
            readiness,
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_valid_utc_dates_to_jst_and_rejects_invalid_timestamps() {
        for (utc, jst) in [
            ("2026-10-23T14:59:59Z", "2026-10-23"),
            ("2026-10-23T15:00:00Z", "2026-10-24"),
            ("2026-01-31T23:59:59Z", "2026-02-01"),
            ("2026-12-31T15:00:00Z", "2027-01-01"),
            ("2024-02-28T15:00:00Z", "2024-02-29"),
            ("2024-02-29T15:00:00Z", "2024-03-01"),
            ("2000-02-28T15:00:00Z", "2000-02-29"),
            ("2100-02-28T15:00:00Z", "2100-03-01"),
        ] {
            assert_eq!(
                IssueDate::parse(utc).map(|date| date.jst_date),
                Some(jst.into())
            );
        }
        for invalid in [
            "",
            "2026-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-00-01T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "2026-10-23T24:00:00Z",
            "2026-10-23T00:60:00Z",
            "2026-10-23T00:00:60Z",
            "2026-1-01T00:00:00Z",
            "2026-10-23T00:00:00Zextra",
            "9999-12-31T15:00:00Z",
        ] {
            assert!(IssueDate::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn sorts_four_columns_in_both_directions_with_input_order_ties() {
        let issues = [
            (42, 2.0, Some("bob"), "2026-10-22T15:00:00Z"),
            (7, 4.0, Some("Alice"), "2026-10-23T00:00:00Z"),
            (3, 2.0, Some("bob"), "2026-10-23T15:00:00Z"),
            (10, 2.0, None, "2026-10-22T00:00:00Z"),
        ]
        .into_iter()
        .enumerate()
        .map(|(fetched_order, (number, score, author, date))| {
            Ok(RankedIssue {
                issue: Issue {
                    number,
                    title: "title".into(),
                    body: String::new(),
                    details: Some(IssueDetails {
                        author: author.map(str::to_owned),
                        assignees: vec![],
                        created_at: IssueDate::parse(date).ok_or("invalid fixture date")?,
                        fetched_order,
                    }),
                },
                score,
                prefix: "feat".into(),
                readiness: Readiness::Yes,
            })
        })
        .collect::<Result<Vec<_>, String>>();
        assert!(issues.is_ok());
        let Ok(mut issues) = issues else { return };
        for (column, ascending, descending) in [
            (
                SortColumn::Importance,
                vec![42, 3, 10, 7],
                vec![7, 42, 3, 10],
            ),
            (SortColumn::Number, vec![3, 7, 10, 42], vec![42, 10, 7, 3]),
            (SortColumn::Author, vec![7, 42, 3, 10], vec![42, 3, 7, 10]),
            (
                SortColumn::CreatedAt,
                vec![10, 42, 7, 3],
                vec![3, 7, 42, 10],
            ),
        ] {
            for (descending, expected) in [(true, descending), (false, ascending)] {
                IssueSort { column, descending }.apply(&mut issues);
                assert_eq!(
                    issues
                        .iter()
                        .map(|issue| issue.issue.number)
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
        IssueSort::default().apply(&mut issues);
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue.issue.number)
                .collect::<Vec<_>>(),
            [7, 42, 3, 10]
        );
        let mut sort = IssueSort::default();
        for (column, descending) in [
            (SortColumn::Importance, false),
            (SortColumn::Importance, true),
            (SortColumn::Author, false),
            (SortColumn::Author, true),
            (SortColumn::Number, false),
        ] {
            sort.toggle(column);
            assert_eq!(sort, IssueSort { column, descending });
        }
    }

    #[test]
    fn searches_all_words_in_title_or_body_and_preserves_order() {
        let issues = [
            (42, "Search 改善", "画面外\n本文の末尾"),
            (7, "別の機能", "search query を確認"),
            (3, "ＳＥＡＲＣＨ", "SEARCH  QUERY"),
        ]
        .map(|(number, title, body)| RankedIssue {
            issue: Issue {
                details: Some(crate::model::test_issue_details()),
                number,
                title: title.into(),
                body: body.into(),
            },
            score: 3.0,
            prefix: "feat".into(),
            readiness: Readiness::Yes,
        });
        for (query, expected) in [
            ("sEaRcH", vec![42, 7, 3]),
            ("本文の末尾", vec![42]),
            ("search query", vec![7, 3]),
            ("QUERY search", vec![7, 3]),
            ("SEARCH  QUERY", vec![7, 3]),
            (" \tsearch　 query \n", vec![7, 3]),
            ("sea que", vec![7, 3]),
            ("改善 画面外", vec![42]),
            ("画面外 改善", vec![42]),
            ("search 改善", vec![42]),
            ("search missing", vec![]),
            ("ＳＥＡＲＣＨ", vec![3]),
            ("Ｓｅａｒｃｈ", vec![]),
            ("改善画面外", vec![]),
            ("missing", vec![]),
            ("", vec![42, 7, 3]),
            (" \t　\n", vec![42, 7, 3]),
        ] {
            assert_eq!(
                search_issues(&issues, query)
                    .iter()
                    .map(|ranked| ranked.issue.number)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(search_issues(&[], ""), []);
    }

    #[test]
    fn next_ready_uses_importance_then_input_order_and_handles_no_candidate() {
        let ranked = rank_issues(
            (1..=5)
                .map(|number| Issue {
                    details: Some(crate::model::test_issue_details()),
                    number,
                    title: String::new(),
                    body: String::new(),
                })
                .collect(),
            vec![
                (4.0, "fix".into(), Readiness::NeedsInformation),
                (3.9, "fix".into(), Readiness::NeedsInvestigation),
                (1.0, "feat".into(), Readiness::Yes),
                (3.6, "fix".into(), Readiness::Yes),
                (3.6, "docs".into(), Readiness::Yes),
            ],
        );
        assert_eq!(
            next_ready_issue(&ranked).map(|item| item.issue.number),
            Some(4)
        );
        let not_ready = ranked
            .into_iter()
            .filter(|item| item.readiness != Readiness::Yes)
            .collect::<Vec<_>>();
        assert!(next_ready_issue(&not_ready).is_none());
        assert!(next_ready_issue(&[]).is_none());
    }

    #[test]
    fn stable_sort_keeps_input_order_for_equal_scores() {
        let ranked = rank_issues(
            vec![
                Issue {
                    details: Some(crate::model::test_issue_details()),
                    number: 42,
                    title: "first".to_owned(),
                    body: String::new(),
                },
                Issue {
                    details: Some(crate::model::test_issue_details()),
                    number: 7,
                    title: "second".to_owned(),
                    body: String::new(),
                },
                Issue {
                    details: Some(crate::model::test_issue_details()),
                    number: 3,
                    title: "third".to_owned(),
                    body: String::new(),
                },
            ],
            vec![
                (2.0, "fix".to_owned(), Readiness::NeedsInvestigation),
                (4.0, "feat".to_owned(), Readiness::NeedsInformation),
                (2.0, "docs".to_owned(), Readiness::Yes),
            ],
        );
        assert_eq!(
            ranked
                .iter()
                .map(|issue| issue.readiness)
                .collect::<Vec<_>>(),
            [
                Readiness::NeedsInformation,
                Readiness::NeedsInvestigation,
                Readiness::Yes
            ]
        );
        assert_eq!(
            ranked
                .iter()
                .map(|item| item.issue.number)
                .collect::<Vec<_>>(),
            vec![7, 42, 3]
        );
    }
}
