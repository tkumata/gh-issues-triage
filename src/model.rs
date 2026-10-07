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
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RankedIssue {
    pub(crate) issue: Issue,
    pub(crate) score: f64,
    pub(crate) prefix: String,
    pub(crate) readiness: Readiness,
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
    fn next_ready_uses_importance_then_input_order_and_handles_no_candidate() {
        let ranked = rank_issues(
            (1..=5)
                .map(|number| Issue {
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
                    number: 42,
                    title: "first".to_owned(),
                    body: String::new(),
                },
                Issue {
                    number: 7,
                    title: "second".to_owned(),
                    body: String::new(),
                },
                Issue {
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
