use reqwest::{blocking::Client, header::ACCEPT};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Issue, RepositoryRef};

const GITHUB_ISSUES_URL: &str = "https://api.github.com/repos";
const MAX_ISSUES: usize = 10;
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

fn append_github_issues(issues: &mut Vec<Issue>, page_items: Vec<GithubIssueResponse>) {
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
            source_order: issues.len(),
        });
    }
}

pub(crate) fn fetch_issues(
    client: &Client,
    token: &str,
    repository: &RepositoryRef,
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
        append_github_issues(&mut issues, page_items);
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
    fn collects_multiple_github_pages_up_to_ten_and_handles_empty() {
        let mut issues = Vec::new();
        append_github_issues(
            &mut issues,
            vec![
                GithubIssueResponse {
                    number: 1,
                    title: "pr".to_owned(),
                    body: None,
                    pull_request: Some(json!({})),
                },
                GithubIssueResponse {
                    number: 2,
                    title: "first issue".to_owned(),
                    body: None,
                    pull_request: None,
                },
            ],
        );
        append_github_issues(
            &mut issues,
            vec![GithubIssueResponse {
                number: 3,
                title: "second issue".to_owned(),
                body: Some("body".to_owned()),
                pull_request: None,
            }],
        );
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue.source_order)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        let ten = (0..12)
            .map(|number| GithubIssueResponse {
                number,
                title: number.to_string(),
                body: None,
                pull_request: None,
            })
            .collect::<Vec<_>>();
        append_github_issues(&mut issues, ten);
        assert_eq!(issues.len(), MAX_ISSUES);
        let mut empty = Vec::new();
        append_github_issues(&mut empty, Vec::new());
        assert!(empty.is_empty());
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
