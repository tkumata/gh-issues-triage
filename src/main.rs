use std::{
    env,
    fmt::{self, Display, Formatter},
    process,
    time::Duration,
};

use reqwest::blocking::Client;

mod auth;
mod cli;
mod credentials;
mod github;
mod jev;
mod model;
mod presentation;

use auth::AuthError;
use cli::{Command, USAGE, parse_args};
use credentials::CredentialError;
use github::{GithubError, fetch_issues};
use jev::{JevError, triage_issues, typesafe_api_key};
use model::RepositoryRef;
use presentation::{DisplayError, render_table, terminal_width};

const USER_AGENT: &str = "gh-issues-triage/0.1.0";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum AppError {
    Auth(AuthError),
    Credential(CredentialError),
    Github(GithubError),
    Jev(JevError),
    Display(DisplayError),
}

impl From<AuthError> for AppError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
impl From<GithubError> for AppError {
    fn from(error: GithubError) -> Self {
        Self::Github(error)
    }
}
impl From<CredentialError> for AppError {
    fn from(error: CredentialError) -> Self {
        Self::Credential(error)
    }
}
impl From<JevError> for AppError {
    fn from(error: JevError) -> Self {
        Self::Jev(error)
    }
}
impl From<DisplayError> for AppError {
    fn from(error: DisplayError) -> Self {
        Self::Display(error)
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(error) => Display::fmt(error, formatter),
            Self::Credential(error) => Display::fmt(error, formatter),
            Self::Github(error) => Display::fmt(error, formatter),
            Self::Jev(error) => Display::fmt(error, formatter),
            Self::Display(error) => Display::fmt(error, formatter),
        }
    }
}

fn http_client() -> Result<Client, AuthError> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|_| AuthError::HttpRequest { timeout: false })
}

fn device_authorize(client: &Client) -> Result<credentials::Credentials, AppError> {
    credentials::ensure_available()?;
    let client_id = auth::client_id()?;
    let device = auth::request_device_code(client, &client_id)?;
    println!(
        "Open {} and enter code {}",
        device.verification_uri, device.user_code
    );
    let tokens = auth::poll_access_token(client, &client_id, &device)?;
    let credentials = credentials::Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    };
    Ok(credentials)
}

fn run_repository(repository: RepositoryRef) -> Result<(), AppError> {
    let client = http_client()?;
    let stored = match credentials::load_credentials() {
        Ok(credentials) => Some(credentials),
        Err(CredentialError::NotLoggedIn) => None,
        Err(error) => return Err(AppError::Credential(error)),
    };
    let stored = match stored {
        Some(credentials) => credentials,
        None => {
            let credentials = device_authorize(&client)?;
            credentials::save_credentials(&credentials)?;
            credentials
        }
    };
    let issues = match fetch_issues(&client, &stored.access_token, &repository) {
        Ok(issues) => issues,
        Err(GithubError::HttpStatus { status: 401 }) => {
            let refreshed = if let Some(refresh_token) = stored.refresh_token.as_deref() {
                let client_id = auth::client_id()?;
                match auth::refresh_access_token(&client, &client_id, refresh_token) {
                    Ok(tokens) => credentials::Credentials {
                        access_token: tokens.access_token,
                        refresh_token: tokens.refresh_token,
                    },
                    Err(AuthError::Failure(auth::AuthFailure::BadRefreshToken)) => {
                        device_authorize(&client)?
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                device_authorize(&client)?
            };
            credentials::save_credentials(&refreshed)?;
            fetch_issues(&client, &refreshed.access_token, &repository)?
        }
        Err(error) => return Err(error.into()),
    };
    let ranked = if issues.is_empty() {
        triage_issues(&client, "", issues)?
    } else {
        triage_issues(&client, &typesafe_api_key()?, issues)?
    };
    println!("{}", render_table(&ranked, terminal_width()?)?);
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(reason) => {
            eprintln!("error: {reason}\n{USAGE}");
            process::exit(2);
        }
    };
    let result = match command {
        Command::Repository(repository) => run_repository(repository),
    };
    if let Err(reason) = result {
        eprintln!("error: {reason}");
        process::exit(1);
    }
}
