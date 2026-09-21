use keyring::Entry;

const CREDENTIAL_SERVICE: &str = "gh-issues-triage";
const CREDENTIAL_USERNAME: &str = "github.com";

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
            Self::NotLoggedIn => {
                formatter.write_str("no GitHub access token is stored; run login first")
            }
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

pub(crate) fn save_token(token: &str) -> Result<(), CredentialError> {
    if token.is_empty() {
        return Err(CredentialError::InvalidResponse);
    }
    entry()?
        .set_password(token)
        .map_err(|_| CredentialError::CredentialWriteFailed)
}

pub(crate) fn load_token() -> Result<String, CredentialError> {
    match entry()?.get_password() {
        Ok(token) if !token.is_empty() => Ok(token),
        Ok(_) => Err(CredentialError::CredentialReadFailed),
        Err(error) => Err(classify_credential_read_error(error)),
    }
}

fn classify_credential_read_error(error: keyring::Error) -> CredentialError {
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
            classify_credential_read_error(keyring::Error::NoEntry),
            CredentialError::NotLoggedIn
        ));
        assert!(matches!(
            classify_credential_read_error(keyring::Error::Invalid(
                "entry".to_owned(),
                "invalid".to_owned()
            )),
            CredentialError::CredentialReadFailed
        ));
    }
}
