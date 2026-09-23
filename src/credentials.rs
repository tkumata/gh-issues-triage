use keyring::Entry;
use serde::{Deserialize, Serialize};

const CREDENTIAL_SERVICE: &str = "gh-issues-triage";
const CREDENTIAL_USERNAME: &str = "github.com";

pub(crate) struct Credentials {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredCredentials {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug)]
pub(crate) enum CredentialError {
    CredentialUnavailable,
    CredentialWriteFailed,
    NotLoggedIn,
    CredentialReadFailed,
    InvalidResponse,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CredentialUnavailable => {
                formatter.write_str("OS credential store is unavailable")
            }
            Self::CredentialWriteFailed => {
                formatter.write_str("could not save GitHub access token")
            }
            Self::NotLoggedIn => formatter.write_str("no GitHub credentials are stored"),
            Self::CredentialReadFailed => formatter.write_str("could not read GitHub access token"),
            Self::InvalidResponse => formatter.write_str("GitHub returned an invalid response"),
        }
    }
}

fn entry() -> Result<Entry, CredentialError> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USERNAME)
        .map_err(|_| CredentialError::CredentialUnavailable)
}

pub(crate) fn ensure_available() -> Result<(), CredentialError> {
    if Entry::store_status().is_err() {
        Err(CredentialError::CredentialUnavailable)
    } else {
        Ok(())
    }
}

pub(crate) fn save_credentials(credentials: &Credentials) -> Result<(), CredentialError> {
    if credentials.access_token.is_empty()
        || credentials
            .refresh_token
            .as_ref()
            .is_some_and(String::is_empty)
    {
        return Err(CredentialError::InvalidResponse);
    }
    let value = serde_json::to_string(&StoredCredentials {
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
    })
    .map_err(|_| CredentialError::InvalidResponse)?;
    entry()?
        .set_password(&value)
        .map_err(|_| CredentialError::CredentialWriteFailed)
}

pub(crate) fn load_credentials() -> Result<Credentials, CredentialError> {
    match entry()?.get_password() {
        Ok(value) if !value.is_empty() => parse_credentials(&value),
        Ok(_) => Err(CredentialError::CredentialReadFailed),
        Err(error) => Err(classify_credential_read_error(&error)),
    }
}

fn parse_credentials(value: &str) -> Result<Credentials, CredentialError> {
    if value.trim_start().starts_with('{') {
        let stored = serde_json::from_str::<StoredCredentials>(value)
            .map_err(|_| CredentialError::CredentialReadFailed)?;
        if stored.access_token.is_empty()
            || stored.refresh_token.as_ref().is_some_and(String::is_empty)
        {
            return Err(CredentialError::CredentialReadFailed);
        }
        return Ok(Credentials {
            access_token: stored.access_token,
            refresh_token: stored.refresh_token,
        });
    }
    // Values saved by earlier versions contain only the access token.
    Ok(Credentials {
        access_token: value.to_owned(),
        refresh_token: None,
    })
}

fn classify_credential_read_error(error: &keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::NotLoggedIn,
        _ => CredentialError::CredentialReadFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_missing_and_unreadable_credentials() {
        assert!(matches!(
            classify_credential_read_error(&keyring::Error::NoEntry),
            CredentialError::NotLoggedIn
        ));
        assert!(matches!(
            classify_credential_read_error(&keyring::Error::Invalid(
                "entry".to_owned(),
                "invalid".to_owned()
            )),
            CredentialError::CredentialReadFailed
        ));
    }

    #[test]
    fn reads_both_credential_formats_without_exposing_tokens() {
        assert!(matches!(
            parse_credentials("gho_legacy"),
            Ok(Credentials { access_token, refresh_token: None }) if access_token == "gho_legacy"
        ));

        assert!(matches!(
            parse_credentials(r#"{"access_token":"gho_access","refresh_token":"ghr_refresh"}"#),
            Ok(Credentials { access_token, refresh_token: Some(refresh_token) })
                if access_token == "gho_access" && refresh_token == "ghr_refresh"
        ));
    }

    #[test]
    fn rejects_corrupt_json_credentials() {
        for value in [
            "{broken",
            r#"{"access_token":"","refresh_token":"ghr_refresh"}"#,
            r#"{"access_token":"gho_access","refresh_token":""}"#,
        ] {
            assert!(matches!(
                parse_credentials(value),
                Err(CredentialError::CredentialReadFailed)
            ));
        }
    }
}
