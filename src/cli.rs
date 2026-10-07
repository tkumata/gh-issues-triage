use crate::model::RepositoryRef;

pub(crate) const USAGE: &str = concat!(
    "gh-issues-triage ",
    env!("CARGO_PKG_VERSION"),
    "\n\n使い方:\n",
    "  gh-issues-triage <owner>/<repo>\n",
    "  gh-issues-triage next <owner>/<repo> --format json\n",
    "  gh-issues-triage config set-root <absolute-directory>\n",
    "  gh-issues-triage --help\n",
    "\n引数:\n",
    "  <owner>/<repo>                 GitHub アカウント / GitHub リポジトリ名\n",
    "  next <owner>/<repo> --format json  着手可能な最重要 Issue を JSON で出力\n",
    "  config set-root <directory>    リポジトリの親ディレクトリ (初回に一度設定が必要)\n",
    "  --help                         このヘルプを表示"
);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Repository(RepositoryRef),
    Next(RepositoryRef),
    SetRoot(std::path::PathBuf),
}

pub(crate) fn parse_args(args: &[String]) -> Result<Command, String> {
    match args {
        [help] if help == "--help" => Ok(Command::Help),
        [] => Err("a command or repository is required".to_owned()),
        [value] => parse_repository(value).map(Command::Repository),
        [next, repository, format, value]
            if next == "next" && format == "--format" && value == "json" =>
        {
            parse_repository(repository).map(Command::Next)
        }
        [config, set_root, directory] if config == "config" && set_root == "set-root" => {
            let path = std::path::PathBuf::from(directory);
            if !path.is_absolute() || !path.is_dir() {
                return Err("root must be an existing absolute directory".to_owned());
            }
            Ok(Command::SetRoot(path))
        }
        _ => Err("exactly one repository is required".to_owned()),
    }
}

fn parse_repository(value: &str) -> Result<RepositoryRef, String> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();

    if owner.is_empty()
        || repo.is_empty()
        || owner == "."
        || owner == ".."
        || repo == "."
        || repo == ".."
        || parts.next().is_some()
    {
        return Err("repository must use the <owner>/<repo> format".to_owned());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("repository must not contain whitespace".to_owned());
    }

    Ok(RepositoryRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
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
    fn accepts_help() {
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert!(parse_args(&args(&["--help", "extra"])).is_err());
    }

    #[test]
    fn accepts_next_json_and_rejects_invalid_next_arguments() {
        assert_eq!(
            parse_args(&args(&["next", "owner/repo", "--format", "json"])),
            Ok(Command::Next(RepositoryRef {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
            }))
        );
        for values in [
            vec!["next"],
            vec!["next", "owner/repo"],
            vec!["next", "owner/repo", "--format"],
            vec!["next", "owner/repo", "--format", "text"],
            vec!["next", "owner/repo", "--unknown", "json"],
            vec!["next", "owner/repo", "--format", "json", "extra"],
            vec!["next", "owner/repo", "--format", "json", "--format", "json"],
            vec!["next", "owner/repo/extra", "--format", "json"],
            vec!["next", "owner/..", "--format", "json"],
            vec!["next", "/repo", "--format", "json"],
            vec!["owner/repo", "--format", "json"],
        ] {
            assert!(parse_args(&args(&values)).is_err(), "{values:?}");
        }
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
            "../repo",
            "owner/..",
            "owner/.",
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
