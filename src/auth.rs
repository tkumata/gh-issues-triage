use std::{
    env,
    fmt::{self, Display, Formatter},
    thread,
    time::{Duration, Instant},
};

use reqwest::{blocking::Client, header::ACCEPT};
use serde::Deserialize;

const CLIENT_ID_ENV: &str = "GITHUB_CLIENT_ID";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

#[derive(PartialEq, Eq)]
pub(crate) struct TokenSet {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenSet")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceCode {
    device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    expires_in: Duration,
    interval: Duration,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum PollDecision {
    Success(TokenSet),
    Pending,
    SlowDown(Option<Duration>),
    Failure(AuthFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthFailure {
    AccessDenied,
    ExpiredToken,
    IncorrectClientCredentials,
    IncorrectDeviceCode,
    DeviceFlowDisabled,
    UnsupportedGrantType,
    BadRefreshToken,
    Unknown,
}

#[derive(Debug)]
pub(crate) enum AuthError {
    MissingClientId,
    HttpRequest { timeout: bool },
    InvalidJson,
    InvalidResponse,
    HttpStatus { status: u16 },
    Failure(AuthFailure),
    Expired,
}

impl Display for AuthFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::AccessDenied => "authorization was denied",
            Self::ExpiredToken => "device code expired",
            Self::IncorrectClientCredentials => "client credentials were rejected",
            Self::IncorrectDeviceCode => "device code was rejected",
            Self::DeviceFlowDisabled => "device flow is disabled for this GitHub App",
            Self::UnsupportedGrantType => "device flow grant type is unsupported",
            Self::BadRefreshToken => "GitHub rejected the saved refresh token",
            Self::Unknown => "GitHub returned an unsupported OAuth error",
        };
        formatter.write_str(message)
    }
}

impl Display for AuthError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingClientId => write!(formatter, "{CLIENT_ID_ENV} is not set"),
            Self::HttpRequest { timeout: true } => formatter.write_str("GitHub request timed out"),
            Self::HttpRequest { timeout: false } => formatter.write_str("GitHub request failed"),
            Self::InvalidJson => formatter.write_str("GitHub returned invalid JSON"),
            Self::InvalidResponse => formatter.write_str("GitHub returned an invalid response"),
            Self::HttpStatus { status } => write!(formatter, "GitHub returned HTTP {status}"),
            Self::Failure(failure) => Display::fmt(failure, formatter),
            Self::Expired => {
                formatter.write_str("device flow expired before authorization completed")
            }
        }
    }
}

pub(crate) fn client_id() -> Result<String, AuthError> {
    env::var(CLIENT_ID_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::MissingClientId)
}

fn parse_device_code(body: &str) -> Result<DeviceCode, AuthError> {
    let response: DeviceCodeResponse =
        serde_json::from_str(body).map_err(|_| AuthError::InvalidJson)?;
    if response.device_code.is_empty()
        || response.user_code.is_empty()
        || response.verification_uri.is_empty()
        || response.expires_in == 0
        || response.interval.unwrap_or(0) == 0
    {
        return Err(AuthError::InvalidResponse);
    }
    Ok(DeviceCode {
        device_code: response.device_code,
        user_code: response.user_code,
        verification_uri: response.verification_uri,
        expires_in: Duration::from_secs(response.expires_in),
        interval: Duration::from_secs(response.interval.unwrap_or_default()),
    })
}

fn parse_token_response(status: u16, body: &str) -> Result<PollDecision, AuthError> {
    let response: TokenResponse = serde_json::from_str(body).map_err(|_| AuthError::InvalidJson)?;
    if !(200..300).contains(&status) {
        return Err(AuthError::HttpStatus { status });
    }
    if let Some(token) = response.access_token {
        if token.is_empty() || response.error.is_some() {
            return Err(AuthError::InvalidResponse);
        }
        return Ok(PollDecision::Success(TokenSet {
            access_token: token,
            refresh_token: response.refresh_token,
        }));
    }
    match response.error.as_deref() {
        Some("authorization_pending") => Ok(PollDecision::Pending),
        Some("slow_down") => Ok(PollDecision::SlowDown(
            response.interval.map(Duration::from_secs),
        )),
        Some("access_denied") => Ok(PollDecision::Failure(AuthFailure::AccessDenied)),
        Some("expired_token") => Ok(PollDecision::Failure(AuthFailure::ExpiredToken)),
        Some("incorrect_client_credentials") => Ok(PollDecision::Failure(
            AuthFailure::IncorrectClientCredentials,
        )),
        Some("incorrect_device_code") => {
            Ok(PollDecision::Failure(AuthFailure::IncorrectDeviceCode))
        }
        Some("device_flow_disabled") => Ok(PollDecision::Failure(AuthFailure::DeviceFlowDisabled)),
        Some("unsupported_grant_type") => {
            Ok(PollDecision::Failure(AuthFailure::UnsupportedGrantType))
        }
        Some("bad_refresh_token") => Ok(PollDecision::Failure(AuthFailure::BadRefreshToken)),
        Some(error) if !error.is_empty() => Ok(PollDecision::Failure(AuthFailure::Unknown)),
        _ => Err(AuthError::InvalidResponse),
    }
}

pub(crate) fn request_device_code(
    client: &Client,
    client_id: &str,
) -> Result<DeviceCode, AuthError> {
    let response = client
        .post(DEVICE_CODE_URL)
        .query(&[("client_id", client_id)])
        .header(ACCEPT, "application/json")
        .send()
        .map_err(|error| AuthError::HttpRequest {
            timeout: error.is_timeout(),
        })?;
    let status = response.status().as_u16();
    let body = response.text().map_err(|error| AuthError::HttpRequest {
        timeout: error.is_timeout(),
    })?;
    if !(200..300).contains(&status) {
        return Err(AuthError::HttpStatus { status });
    }
    parse_device_code(&body)
}

fn next_interval(current: Duration, returned: Option<Duration>) -> Duration {
    current
        .checked_add(Duration::from_secs(5))
        .unwrap_or(Duration::MAX)
        .max(returned.unwrap_or_default())
}

fn poll_wait(now: Instant, deadline: Instant, interval: Duration) -> Option<Duration> {
    (now < deadline).then(|| interval.min(deadline.saturating_duration_since(now)))
}

pub(crate) fn poll_access_token(
    client: &Client,
    client_id: &str,
    device: &DeviceCode,
) -> Result<TokenSet, AuthError> {
    let started = Instant::now();
    let deadline = started
        .checked_add(device.expires_in)
        .ok_or(AuthError::Expired)?;
    let mut interval = device.interval;
    loop {
        let wait = poll_wait(Instant::now(), deadline, interval).ok_or(AuthError::Expired)?;
        thread::sleep(wait);
        if Instant::now() >= deadline {
            return Err(AuthError::Expired);
        }
        let response = client
            .post(ACCESS_TOKEN_URL)
            .query(&[
                ("client_id", client_id),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .header(ACCEPT, "application/json")
            .send()
            .map_err(|error| AuthError::HttpRequest {
                timeout: error.is_timeout(),
            })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|error| AuthError::HttpRequest {
            timeout: error.is_timeout(),
        })?;
        match parse_token_response(status, &body)? {
            PollDecision::Success(token) => return Ok(token),
            PollDecision::Pending => {}
            PollDecision::SlowDown(returned) => interval = next_interval(interval, returned),
            PollDecision::Failure(failure) => return Err(AuthError::Failure(failure)),
        }
    }
}

pub(crate) fn refresh_access_token(
    client: &Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenSet, AuthError> {
    let response = client
        .post(ACCESS_TOKEN_URL)
        .query(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .header(ACCEPT, "application/json")
        .send()
        .map_err(|error| AuthError::HttpRequest {
            timeout: error.is_timeout(),
        })?;
    let status = response.status().as_u16();
    let body = response.text().map_err(|error| AuthError::HttpRequest {
        timeout: error.is_timeout(),
    })?;
    parse_refresh_response(status, &body)
}

fn parse_refresh_response(status: u16, body: &str) -> Result<TokenSet, AuthError> {
    match parse_token_response(status, body)? {
        PollDecision::Success(tokens)
            if tokens
                .refresh_token
                .as_ref()
                .is_some_and(|refresh_token| !refresh_token.is_empty()) =>
        {
            Ok(tokens)
        }
        PollDecision::Success(_) => Err(AuthError::InvalidResponse),
        PollDecision::Failure(failure) => Err(AuthError::Failure(failure)),
        PollDecision::Pending | PollDecision::SlowDown(_) => Err(AuthError::InvalidResponse),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_code_response() {
        let response = parse_device_code(r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#).unwrap();
        assert_eq!(response.interval, Duration::from_secs(5));
        assert_eq!(response.expires_in, Duration::from_secs(900));
    }

    #[test]
    fn parses_token_state_transitions_without_storing_secrets() {
        assert_eq!(
            parse_token_response(200, r#"{"error":"authorization_pending"}"#).unwrap(),
            PollDecision::Pending
        );
        assert_eq!(
            parse_token_response(200, r#"{"error":"slow_down","interval":20}"#).unwrap(),
            PollDecision::SlowDown(Some(Duration::from_secs(20)))
        );
    }

    #[test]
    fn parses_token_success_and_distinguishes_http_and_json_errors() {
        assert_eq!(
            parse_token_response(
                200,
                r#"{"access_token":"token-secret","token_type":"bearer"}"#
            )
            .unwrap(),
            PollDecision::Success(TokenSet {
                access_token: "token-secret".to_owned(),
                refresh_token: None,
            })
        );
        assert!(matches!(
            parse_token_response(500, r#"{"error":"device_flow_disabled"}"#),
            Err(AuthError::HttpStatus { status: 500 })
        ));
        assert!(matches!(
            parse_token_response(200, "not-json"),
            Err(AuthError::InvalidJson)
        ));
    }

    #[test]
    fn requires_rotated_tokens_and_classifies_bad_refresh_token() {
        assert_eq!(
            parse_refresh_response(
                200,
                r#"{"access_token":"new-access","refresh_token":"new-refresh"}"#
            )
            .unwrap(),
            TokenSet {
                access_token: "new-access".to_owned(),
                refresh_token: Some("new-refresh".to_owned()),
            }
        );
        for body in [
            r#"{"access_token":"new-access"}"#,
            r#"{"access_token":"new-access","refresh_token":""}"#,
        ] {
            assert!(matches!(
                parse_refresh_response(200, body),
                Err(AuthError::InvalidResponse)
            ));
        }
        assert!(matches!(
            parse_refresh_response(200, r#"{"error":"bad_refresh_token"}"#),
            Err(AuthError::Failure(AuthFailure::BadRefreshToken))
        ));
    }

    #[test]
    fn slows_down_by_five_seconds_or_server_interval() {
        assert_eq!(
            next_interval(Duration::from_secs(5), None),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_interval(Duration::from_secs(5), Some(Duration::from_secs(20))),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn classifies_known_and_unknown_terminal_oauth_errors() {
        for (code, expected) in [
            ("access_denied", AuthFailure::AccessDenied),
            ("expired_token", AuthFailure::ExpiredToken),
            (
                "incorrect_client_credentials",
                AuthFailure::IncorrectClientCredentials,
            ),
            ("incorrect_device_code", AuthFailure::IncorrectDeviceCode),
            ("device_flow_disabled", AuthFailure::DeviceFlowDisabled),
            ("unsupported_grant_type", AuthFailure::UnsupportedGrantType),
            ("bad_refresh_token", AuthFailure::BadRefreshToken),
        ] {
            assert_eq!(
                parse_token_response(200, &format!(r#"{{"error":"{code}"}}"#)).unwrap(),
                PollDecision::Failure(expected)
            );
        }
        assert_eq!(
            parse_token_response(200, r#"{"error":"external-secret-description"}"#).unwrap(),
            PollDecision::Failure(AuthFailure::Unknown)
        );
    }

    #[test]
    fn rejects_expired_or_incomplete_device_code() {
        assert!(parse_device_code(r#"{"device_code":"x","user_code":"y","verification_uri":"https://github.com/login/device","expires_in":0,"interval":5}"#).is_err());
        assert!(parse_token_response(200, r#"{"error":"expired_token"}"#).is_ok());
        let now = Instant::now();
        assert_eq!(poll_wait(now, now, Duration::from_secs(5)), None);
    }

    #[test]
    fn poll_wait_stops_at_deadline() {
        let now = Instant::now();
        assert_eq!(poll_wait(now, now, Duration::from_secs(5)), None);
    }
}
