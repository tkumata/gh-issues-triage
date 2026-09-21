use std::{collections::BTreeMap, fmt, thread, time::Duration};

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::model::{Issue, RankedIssue, SCORE_CRITERIA, rank_issues};

const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";
const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";
const TYPESAFE_MODEL: &str = "jev-latest";
const JEV_MAX_ATTEMPTS: u32 = 3;
const JEV_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
#[derive(Debug)]
pub(crate) enum JevError {
    MissingApiKey,
    HttpRequest { timeout: bool },
    HttpStatus { status: u16 },
    InvalidJson,
    InvalidResponse,
}

impl fmt::Display for JevError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingApiKey => write!(formatter, "{TYPESAFE_API_KEY_ENV} is not set"),
            Self::HttpRequest { timeout: true } => {
                formatter.write_str("TypeSafe request timed out")
            }
            Self::HttpRequest { timeout: false } => formatter.write_str("TypeSafe request failed"),
            Self::HttpStatus { status } => write!(formatter, "TypeSafe returned HTTP {status}"),
            Self::InvalidJson => formatter.write_str("TypeSafe returned invalid JSON"),
            Self::InvalidResponse => formatter.write_str("TypeSafe returned an invalid response"),
        }
    }
}

pub(crate) fn typesafe_api_key() -> Result<String, JevError> {
    std::env::var(TYPESAFE_API_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(JevError::MissingApiKey)
}

pub(crate) fn build_jev_request(issues: &[Issue]) -> Value {
    let state = json!({ "issues": issues.iter().map(|issue| json!({ "number": issue.number, "title": issue.title, "body": issue.body })).collect::<Vec<_>>() });
    let questions = issues.iter().enumerate().map(|(index, _)| {
        let id = format!("issue_{index}");
        let instructions = format!("Judge the importance of the issue at `issues[{index}].number`, `issues[{index}].title`, and `issues[{index}].body` in the state. Use those fields to determine the issue's importance.");
        (id, json!({ "type": "score", "instructions": instructions, "criteria": SCORE_CRITERIA }))
    }).collect::<BTreeMap<_, _>>();
    json!({"model": TYPESAFE_MODEL, "state": state, "questions": questions})
}

fn retry_jev_status(status: u16) -> bool {
    matches!(status, 429 | 529)
}

fn jev_retry_delay(attempt: u32) -> Duration {
    JEV_INITIAL_BACKOFF
        .checked_mul(2u32.saturating_pow(attempt))
        .unwrap_or(Duration::MAX)
}

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn validate_score_answer(answer: &Value) -> Result<f64, JevError> {
    let object = answer.as_object().ok_or(JevError::InvalidResponse)?;
    if object.get("type").and_then(Value::as_str) != Some("score") {
        return Err(JevError::InvalidResponse);
    }
    let score = number(object.get("score")).ok_or(JevError::InvalidResponse)?;
    let confidence = number(object.get("confidence")).ok_or(JevError::InvalidResponse)?;
    if !(0.0..=4.0).contains(&score) || !(0.0..=1.0).contains(&confidence) {
        return Err(JevError::InvalidResponse);
    }
    let legend = object
        .get("legend")
        .and_then(Value::as_object)
        .ok_or(JevError::InvalidResponse)?;
    let probabilities = object
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(JevError::InvalidResponse)?;
    if legend.len() != SCORE_CRITERIA.len() || probabilities.len() != SCORE_CRITERIA.len() {
        return Err(JevError::InvalidResponse);
    }
    let mut sum = 0.0;
    let mut weighted_score = 0.0;
    for (index, criterion) in SCORE_CRITERIA.iter().enumerate() {
        let key = index.to_string();
        if legend.get(&key).and_then(Value::as_str) != Some(*criterion) {
            return Err(JevError::InvalidResponse);
        }
        let probability = number(probabilities.get(&key)).ok_or(JevError::InvalidResponse)?;
        if !(0.0..=1.0).contains(&probability) {
            return Err(JevError::InvalidResponse);
        }
        sum += probability;
        weighted_score += probability * index as f64;
    }
    if (sum - 1.0).abs() > 1e-6 || (weighted_score - score).abs() > 1e-6 {
        return Err(JevError::InvalidResponse);
    }
    Ok(score)
}

fn parse_jev_answers(body: &str, issue_count: usize) -> Result<Vec<f64>, JevError> {
    let response: Value = serde_json::from_str(body).map_err(|_| JevError::InvalidJson)?;
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(JevError::InvalidResponse)?;
    if answers.len() != issue_count {
        return Err(JevError::InvalidResponse);
    }
    (0..issue_count)
        .map(|index| {
            let id = format!("issue_{index}");
            validate_score_answer(answers.get(&id).ok_or(JevError::InvalidResponse)?)
                .map_err(|_| JevError::InvalidResponse)
        })
        .collect()
}

pub(crate) fn triage_issues(
    client: &Client,
    api_key: &str,
    issues: Vec<Issue>,
) -> Result<Vec<RankedIssue>, JevError> {
    if issues.is_empty() {
        return Ok(Vec::new());
    }
    let payload = build_jev_request(&issues);
    for attempt in 0..JEV_MAX_ATTEMPTS {
        let response = client
            .post(TYPESAFE_URL)
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .map_err(|error| JevError::HttpRequest {
                timeout: error.is_timeout(),
            })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|error| JevError::HttpRequest {
            timeout: error.is_timeout(),
        })?;
        if !(200..300).contains(&status) {
            if retry_jev_status(status) && attempt + 1 < JEV_MAX_ATTEMPTS {
                thread::sleep(jev_retry_delay(attempt));
                continue;
            }
            return Err(JevError::HttpStatus { status });
        }
        let issue_count = issues.len();
        let scores = parse_jev_answers(&body, issue_count)?;
        return Ok(rank_issues(issues, scores));
    }
    Err(JevError::HttpStatus { status: 429 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, title: &str, body: &str, source_order: usize) -> Issue {
        Issue {
            number,
            title: title.to_owned(),
            body: body.to_owned(),
            source_order,
        }
    }

    fn valid_answer(score: f64) -> Value {
        let bounded_score = score.clamp(0.0, 4.0);
        let lower = bounded_score.floor() as usize;
        let upper = bounded_score.ceil() as usize;
        let mut probabilities = [0.0; 5];
        if lower == upper {
            probabilities[lower] = 1.0;
        } else {
            probabilities[lower] = upper as f64 - bounded_score;
            probabilities[upper] = bounded_score - lower as f64;
        }
        json!({ "type": "score", "score": score, "confidence": 0.8, "legend": { "0": SCORE_CRITERIA[0], "1": SCORE_CRITERIA[1], "2": SCORE_CRITERIA[2], "3": SCORE_CRITERIA[3], "4": SCORE_CRITERIA[4] }, "probabilities": { "0": probabilities[0], "1": probabilities[1], "2": probabilities[2], "3": probabilities[3], "4": probabilities[4] } })
    }

    #[test]
    fn builds_one_score_question_per_issue() {
        let request =
            build_jev_request(&[issue(42, "broken", "details", 0), issue(7, "old", "", 1)]);
        assert_eq!(request["model"], TYPESAFE_MODEL);
        assert_eq!(request["questions"].as_object().unwrap().len(), 2);
        assert_eq!(request["questions"]["issue_0"]["type"], "score");
        assert_eq!(
            request["questions"]["issue_0"]["criteria"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert!(
            request["questions"]["issue_0"]["instructions"]
                .as_str()
                .unwrap()
                .contains("issues[0].number")
        );
        assert_eq!(request["state"]["issues"][0]["number"], 42);
    }

    #[test]
    fn empty_triage_succeeds_without_an_api_key() {
        assert!(
            triage_issues(&Client::new(), "", Vec::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn validates_score_edges_and_rejects_bad_answers() {
        for score in [0.0, 4.0] {
            assert_eq!(validate_score_answer(&valid_answer(score)).unwrap(), score);
        }
        for answer in [json!({"type":"choice"}), valid_answer(4.1), {
            let mut value = valid_answer(2.0);
            value["confidence"] = json!(1.1);
            value
        }] {
            assert!(validate_score_answer(&answer).is_err());
        }
        let mut wrong_legend = valid_answer(2.0);
        wrong_legend["legend"]["2"] = json!("different");
        assert!(validate_score_answer(&wrong_legend).is_err());
        let mut wrong_probabilities = valid_answer(2.0);
        wrong_probabilities["probabilities"]["3"] = json!(0.9);
        assert!(validate_score_answer(&wrong_probabilities).is_err());
        let mut wrong_weighted_score = valid_answer(2.0);
        wrong_weighted_score["score"] = json!(3.0);
        assert!(validate_score_answer(&wrong_weighted_score).is_err());
    }

    #[test]
    fn rejects_missing_extra_and_wrong_type_answers() {
        let mut answers = serde_json::Map::new();
        answers.insert("issue_0".to_owned(), valid_answer(2.0));
        assert!(parse_jev_answers(&json!({"answers": answers}).to_string(), 2).is_err());
        answers.insert("issue_1".to_owned(), json!({"type":"choice"}));
        answers.insert("extra".to_owned(), valid_answer(1.0));
        assert!(parse_jev_answers(&json!({"answers": answers}).to_string(), 2).is_err());
    }

    #[test]
    fn accepts_only_retryable_jev_statuses_and_caps_delay() {
        assert!(retry_jev_status(429));
        assert!(retry_jev_status(529));
        assert!(!retry_jev_status(500));
        assert_eq!(jev_retry_delay(0), Duration::from_millis(100));
        assert_eq!(jev_retry_delay(1), Duration::from_millis(200));
        assert!(JEV_MAX_ATTEMPTS <= 3);
    }
}
