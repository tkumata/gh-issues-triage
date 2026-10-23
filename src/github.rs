use reqwest::{blocking::Client, header::ACCEPT};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::model::{Issue, IssueDate, IssueDetails, RepositoryRef};

const GITHUB_ISSUES_URL: &str = "https://api.github.com/repos";
const MAX_ISSUES: usize = 100;
const GITHUB_PAGE_SIZE: u32 = 100;

#[derive(Debug)]
pub(crate) enum GithubError {
    HttpRequest { timeout: bool },
    HttpStatus { status: u16 },
    InvalidJson,
    InvalidResponse,
}

impl std::fmt::Display for GithubError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpRequest { timeout: true } => {
                formatter.write_str("GitHub issues request timed out")
            }
            Self::HttpRequest { timeout: false } => {
                formatter.write_str("GitHub issues request failed")
            }
            Self::HttpStatus { status } => {
                if *status == 404 {
                    formatter.write_str(
                        "GitHub repository was not found or the saved token has no access; install the GitHub App on the repository (HTTP 404)",
                    )
                } else {
                    write!(formatter, "GitHub issues returned HTTP {status}")
                }
            }
            Self::InvalidJson => formatter.write_str("GitHub issues returned invalid JSON"),
            Self::InvalidResponse => {
                formatter.write_str("GitHub issues returned an invalid response")
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubIssueResponse {
    number: u64,
    title: String,
    body: Option<String>,
    pull_request: Option<Value>,
    #[serde(flatten)]
    details: Map<String, Value>,
}

fn parse_issue_details(
    fields: &Map<String, Value>,
    fetched_order: usize,
) -> Result<IssueDetails, GithubError> {
    let login = |user: &Value| {
        user.get("login")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(GithubError::InvalidResponse)
    };
    let author = match fields.get("user") {
        Some(Value::Null) => None,
        Some(user) => Some(login(user)?),
        None => return Err(GithubError::InvalidResponse),
    };
    let mut assignees = fields
        .get("assignees")
        .and_then(Value::as_array)
        .ok_or(GithubError::InvalidResponse)?
        .iter()
        .map(login)
        .collect::<Result<Vec<_>, _>>()?;
    assignees.sort();
    let created_at = fields
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(IssueDate::parse)
        .ok_or(GithubError::InvalidResponse)?;
    Ok(IssueDetails {
        author,
        assignees,
        created_at,
        fetched_order,
    })
}

fn parse_github_issues_page(
    status: u16,
    body: &str,
) -> Result<Vec<GithubIssueResponse>, GithubError> {
    if !(200..300).contains(&status) {
        return Err(GithubError::HttpStatus { status });
    }
    serde_json::from_str(body).map_err(|error| {
        if error.is_data() {
            GithubError::InvalidResponse
        } else {
            GithubError::InvalidJson
        }
    })
}

fn percent_encode_path(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                vec![byte as char]
            } else {
                format!("%{byte:02X}").chars().collect()
            }
        })
        .collect()
}

fn github_issues_page_url(repository: &RepositoryRef, page: u32) -> String {
    format!(
        "{GITHUB_ISSUES_URL}/{}/{}/issues?state=open&sort=created&direction=desc&per_page={GITHUB_PAGE_SIZE}&page={page}",
        percent_encode_path(&repository.owner),
        percent_encode_path(&repository.repo)
    )
}

fn append_github_issues(
    issues: &mut Vec<Issue>,
    page_items: Vec<GithubIssueResponse>,
    include_details: bool,
) -> Result<(), GithubError> {
    for item in page_items {
        if issues.len() == MAX_ISSUES {
            break;
        }
        if item.pull_request.is_some() {
            continue;
        }
        issues.push(Issue {
            number: item.number,
            title: item.title,
            body: item.body.unwrap_or_default(),
            details: if include_details {
                Some(parse_issue_details(&item.details, issues.len())?)
            } else {
                None
            },
        });
    }
    Ok(())
}

pub(crate) fn fetch_issues(
    client: &Client,
    token: &str,
    repository: &RepositoryRef,
    include_details: bool,
) -> Result<Vec<Issue>, GithubError> {
    let mut issues = Vec::with_capacity(MAX_ISSUES);
    let mut page = 1;
    while issues.len() < MAX_ISSUES {
        let response = client
            .get(github_issues_page_url(repository, page))
            .bearer_auth(token)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .map_err(|error| GithubError::HttpRequest {
                timeout: error.is_timeout(),
            })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|error| GithubError::HttpRequest {
            timeout: error.is_timeout(),
        })?;
        let page_items = parse_github_issues_page(status, &body)?;
        let page_len = page_items.len();
        append_github_issues(&mut issues, page_items, include_details)?;
        if page_len < GITHUB_PAGE_SIZE as usize {
            break;
        }
        page = page.checked_add(1).ok_or(GithubError::InvalidResponse)?;
    }
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata_response() -> Value {
        json!({"number":42,"title":"issue","body":null,
            "user":{"login":"author"},"assignees":[{"login":"zoe"},{"login":"alice"}],
            "created_at":"2026-12-31T15:00:00Z"})
    }

    fn collect_response(value: &Value, include_details: bool) -> Result<Vec<Issue>, GithubError> {
        let items = parse_github_issues_page(
            200,
            &json!([value,
                {"number":99,"title":"pr","body":null,"pull_request":{},"user":false}
            ])
            .to_string(),
        )?;
        let mut issues = Vec::new();
        append_github_issues(&mut issues, items, include_details)?;
        Ok(issues)
    }

    #[test]
    fn validates_tui_metadata_and_skips_pull_request_metadata() {
        let mut valid = metadata_response();
        let issues = collect_response(&valid, true);
        assert!(issues.is_ok());
        let Ok(issues) = issues else { return };
        let details = issues.first().and_then(|issue| issue.details.as_ref());
        assert!(
            details.is_some_and(|details| details.author.as_deref() == Some("author")
                && details.assignees == ["alice", "zoe"]
                && details.created_at.jst_date == "2027-01-01"
                && details.fetched_order == 0)
        );
        assert_eq!(issues.len(), 1);
        let object = valid.as_object_mut();
        assert!(object.is_some());
        let Some(object) = object else { return };
        object.insert("user".into(), Value::Null);
        object.insert("assignees".into(), json!([]));
        assert!(collect_response(&valid, true).is_ok_and(|issues| {
            issues
                .first()
                .and_then(|issue| issue.details.as_ref())
                .is_some_and(|details| details.author.is_none() && details.assignees.is_empty())
        }));
    }

    #[test]
    fn new_metadata_errors_do_not_change_next_selection_or_json() {
        for field in ["user", "assignees", "created_at"] {
            for replacement in [None, Some(json!(false)), Some(json!({"login":1}))] {
                let mut invalid = metadata_response();
                let object = invalid.as_object_mut();
                assert!(object.is_some());
                let Some(object) = object else { return };
                if let Some(replacement) = replacement {
                    object.insert(field.into(), replacement);
                } else {
                    object.remove(field);
                }
                assert!(matches!(
                    collect_response(&invalid, true),
                    Err(GithubError::InvalidResponse)
                ));
                let issues = collect_response(&invalid, false);
                assert!(issues.is_ok());
                let Ok(issues) = issues else { return };
                assert!(
                    issues
                        .first()
                        .is_some_and(|issue| issue.number == 42 && issue.details.is_none())
                );
                let ranked = crate::model::rank_issues(
                    issues,
                    vec![(3.0, "feat".into(), crate::model::Readiness::Yes)],
                );
                let output = crate::presentation::render_next_json(
                    &RepositoryRef {
                        owner: "owner".into(),
                        repo: "repo".into(),
                    },
                    crate::model::next_ready_issue(&ranked),
                );
                assert!(output.is_ok_and(
                    |output| output.contains("\"number\":42") && !output.contains("created_at")
                ));
            }
        }
        let mut invalid_date = metadata_response();
        let date = invalid_date.get_mut("created_at");
        assert!(date.is_some());
        let Some(date) = date else { return };
        *date = json!("2026-02-29T00:00:00Z");
        assert!(matches!(
            collect_response(&invalid_date, true),
            Err(GithubError::InvalidResponse)
        ));
        assert!(collect_response(&invalid_date, false).is_ok());
    }

    #[test]
    fn parses_github_pages_and_excludes_pull_requests() {
        assert!(parse_github_issues_page(
            200,
            r#"[{"number":1,"title":"issue","body":null},{"number":2,"title":"pr","body":"x","pull_request":{"url":"x"}}]"#
        )
        .is_ok_and(|items| {
            items.len() == 2
                && items
                    .iter()
                    .filter(|item| item.pull_request.is_none())
                    .count()
                    == 1
                && items.first().is_some_and(|issue| {
                    issue.number == 1 && issue.title == "issue" && issue.body.is_none()
                })
        }));
        assert!(matches!(
            parse_github_issues_page(403, "[]"),
            Err(GithubError::HttpStatus { status: 403 })
        ));
        assert!(matches!(
            parse_github_issues_page(200, "not-json"),
            Err(GithubError::InvalidJson)
        ));
        assert!(matches!(
            parse_github_issues_page(200, "{}"),
            Err(GithubError::InvalidResponse)
        ));
    }

    #[test]
    fn explains_missing_repository_or_token_access_for_not_found() {
        let message = GithubError::HttpStatus { status: 404 }.to_string();
        assert!(message.contains("repository was not found"));
        assert!(message.contains("saved token has no access"));
        assert!(message.contains("install the GitHub App"));
    }

    #[test]
    fn collects_multiple_github_pages_up_to_one_hundred_and_handles_empty() {
        let fields = Map::from_iter([
            ("user".into(), json!({"login":"author"})),
            ("assignees".into(), json!([])),
            ("created_at".into(), json!("2026-10-23T00:00:00Z")),
        ]);
        let mut issues = Vec::new();
        assert!(
            append_github_issues(
                &mut issues,
                vec![
                    GithubIssueResponse {
                        details: fields.clone(),
                        number: 1,
                        title: "pr".to_owned(),
                        body: None,
                        pull_request: Some(json!({})),
                    },
                    GithubIssueResponse {
                        details: fields.clone(),
                        number: 2,
                        title: "first issue".to_owned(),
                        body: None,
                        pull_request: None,
                    },
                ],
                true
            )
            .is_ok()
        );
        assert!(
            append_github_issues(
                &mut issues,
                vec![GithubIssueResponse {
                    details: fields.clone(),
                    number: 3,
                    title: "second issue".to_owned(),
                    body: Some("body".to_owned()),
                    pull_request: None,
                }],
                true
            )
            .is_ok()
        );
        assert_eq!(
            issues.iter().map(|issue| issue.number).collect::<Vec<_>>(),
            [2, 3]
        );
        let remaining = (4..114)
            .map(|number| GithubIssueResponse {
                details: fields.clone(),
                number,
                title: number.to_string(),
                body: None,
                pull_request: None,
            })
            .collect::<Vec<_>>();
        assert!(append_github_issues(&mut issues, remaining, true).is_ok());
        assert_eq!(issues.len(), 100);
        assert_eq!(
            issues
                .iter()
                .filter_map(|issue| issue.details.as_ref().map(|details| details.fetched_order))
                .collect::<Vec<_>>(),
            (0..100).collect::<Vec<_>>()
        );
        assert_eq!(
            issues.iter().map(|issue| issue.number).collect::<Vec<_>>(),
            (2..102).collect::<Vec<_>>()
        );
        assert_eq!(issues.first().map(|issue| issue.body.as_str()), Some(""));
        assert_eq!(issues.get(1).map(|issue| issue.body.as_str()), Some("body"));
        let mut empty = Vec::new();
        assert!(append_github_issues(&mut empty, Vec::new(), true).is_ok());
        assert_eq!(empty, [] as [Issue; 0]);
    }

    #[test]
    fn builds_github_request_shape() {
        let url = github_issues_page_url(
            &RepositoryRef {
                owner: "octo-org".to_owned(),
                repo: "widgets".to_owned(),
            },
            2,
        );
        assert_eq!(
            url,
            "https://api.github.com/repos/octo-org/widgets/issues?state=open&sort=created&direction=desc&per_page=100&page=2"
        );
    }
}
