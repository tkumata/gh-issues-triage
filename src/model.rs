use std::cmp::Ordering;

pub(crate) const SCORE_CRITERIA: [&str; 5] = [
    "対応不要、情報提供のみ、または影響が確認できない",
    "影響が小さく、容易な回避策がある",
    "通常機能に影響するが、業務を止めない",
    "主要機能を妨げる、広い利用者に影響する、または回避が困難",
    "データ損失、セキュリティ、サービス停止など直ちに対応すべき影響",
];

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
    pub(crate) source_order: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RankedIssue {
    pub(crate) issue: Issue,
    pub(crate) score: f64,
}

pub(crate) fn rank_issues(issues: Vec<Issue>, scores: Vec<f64>) -> Vec<RankedIssue> {
    let mut ranked = issues
        .into_iter()
        .zip(scores)
        .map(|(issue, score)| RankedIssue { issue, score })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.issue.source_order.cmp(&right.issue.source_order))
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_sort_keeps_source_order_for_equal_scores() {
        let ranked = rank_issues(
            vec![
                Issue {
                    number: 1,
                    title: "first".to_owned(),
                    body: String::new(),
                    source_order: 0,
                },
                Issue {
                    number: 2,
                    title: "second".to_owned(),
                    body: String::new(),
                    source_order: 1,
                },
                Issue {
                    number: 3,
                    title: "third".to_owned(),
                    body: String::new(),
                    source_order: 2,
                },
            ],
            vec![2.0, 4.0, 2.0],
        );
        assert_eq!(
            ranked
                .iter()
                .map(|item| item.issue.number)
                .collect::<Vec<_>>(),
            vec![2, 1, 3]
        );
    }
}
