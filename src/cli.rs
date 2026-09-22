use crate::model::RepositoryRef;

pub(crate) const USAGE: &str = "Usage: gh-issues-triage <owner>/<repo>";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Repository(RepositoryRef),
}

pub(crate) fn parse_args(args: &[String]) -> Result<Command, String> {
    match args {
        [] => Err("a command or repository is required".to_owned()),
        [value] => parse_repository(value),
        _ => Err("exactly one repository is required".to_owned()),
    }
}

fn parse_repository(value: &str) -> Result<Command, String> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();

    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err("repository must use the <owner>/<repo> format".to_owned());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("repository must not contain whitespace".to_owned());
    }

    Ok(Command::Repository(RepositoryRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn accepts_repository() {
        assert_eq!(
            parse_args(&args(&["octo-org/widgets"])),
            Ok(Command::Repository(RepositoryRef {
                owner: "octo-org".to_owned(),
                repo: "widgets".to_owned()
            }))
        );
    }

    #[test]
    fn rejects_invalid_repositories() {
        for value in [
            "login",
            "logout",
            "/repo",
            "owner/",
            "owner/repo/extra",
            "owner//repo",
        ] {
            assert!(parse_args(&args(&[value])).is_err(), "{value}");
        }
    }

    #[test]
    fn rejects_missing_or_extra_arguments() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["login", "extra"])).is_err());
    }
}
