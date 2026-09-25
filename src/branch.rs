use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::model::{RankedIssue, RepositoryRef};

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    root: PathBuf,
}

fn config_path() -> Result<PathBuf, String> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(".config"),
    };
    Ok(base.join("gh-issues-triage/config.json"))
}

pub(crate) fn set_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() || !root.is_dir() {
        return Err("root must be an existing absolute directory".to_owned());
    }
    write_config(&config_path()?, root)
}

fn write_config(path: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("invalid config path")?)
        .map_err(|error| format!("cannot create config directory: {error}"))?;
    let json = serde_json::to_vec(&Config {
        root: root.to_path_buf(),
    })
    .map_err(|error| format!("cannot encode config: {error}"))?;
    fs::write(path, json).map_err(|error| format!("cannot save config: {error}"))
}

fn root() -> Result<PathBuf, String> {
    read_config(&config_path()?)
}

fn read_config(path: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(path).map_err(|error| format!("root is not configured: {error}"))?;
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid config: {error}"))?;
    if !config.root.is_absolute() || !config.root.is_dir() {
        return Err("configured root is not an existing absolute directory".to_owned());
    }
    Ok(config.root)
}

pub(crate) fn branch_name(issue: &RankedIssue) -> String {
    format!("{}/issue-{}", issue.prefix, issue.issue.number)
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn github_remote_matches(remote: &str, repository: &RepositoryRef) -> bool {
    let path = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("http://github.com/"))
        .or_else(|| {
            remote
                .strip_prefix("git@github.com:")
                .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        });
    path.is_some_and(|path| {
        path.trim_end_matches(".git").trim_end_matches('/')
            == format!("{}/{}", repository.owner, repository.repo)
    })
}

pub(crate) fn create_branch(
    repository: &RepositoryRef,
    issue: &RankedIssue,
) -> Result<String, String> {
    create_branch_at(&root()?, repository, issue)
}

fn create_branch_at(
    root: &Path,
    repository: &RepositoryRef,
    issue: &RankedIssue,
) -> Result<String, String> {
    if repository.repo == "." || repository.repo == ".." {
        return Err("repository name is not a valid directory name".to_owned());
    }
    let directory = root.join(&repository.repo);
    if !directory.is_dir() {
        return Err(format!(
            "repository directory does not exist: {}",
            directory.display()
        ));
    }
    let top_level = git(&directory, &["rev-parse", "--show-toplevel"])?;
    let top_level = PathBuf::from(top_level)
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository root: {error}"))?;
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository directory: {error}"))?;
    if top_level != directory {
        return Err("selected directory is not a Git repository root".to_owned());
    }
    let remote = git(&directory, &["config", "--get", "remote.origin.url"])?;
    if !github_remote_matches(&remote, repository) {
        return Err("remote.origin does not match the requested GitHub repository".to_owned());
    }
    git(
        &directory,
        &["show-ref", "--verify", "--quiet", "refs/heads/main"],
    )
    .map_err(|_| "local main branch does not exist".to_owned())?;
    let name = branch_name(issue);
    git(&directory, &["check-ref-format", "--branch", &name])
        .map_err(|_| "generated branch name is invalid".to_owned())?;
    git(&directory, &["checkout", "--no-track", "-b", &name, "main"])
        .map_err(|error| format!("could not create branch: {error}"))?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> std::io::Result<PathBuf> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = env::temp_dir().join(format!("gh-issues-triage-{}-{nonce}", process::id()));
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn run(repo: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
        git(repo, args)
            .map_err(std::io::Error::other)
            .map_err(Into::into)
    }

    fn check(condition: bool, message: &str) -> std::io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(std::io::Error::other(message))
        }
    }

    #[test]
    fn creates_from_main_checks_out_new_branch_and_rejects_duplicate_without_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir()?;
        let repo = root.join("repo");
        fs::create_dir(&repo)?;
        run(&repo, &["init", "--initial-branch=main"])?;
        run(&repo, &["config", "user.email", "test@example.com"])?;
        run(&repo, &["config", "user.name", "Test"])?;
        fs::write(repo.join("file"), "main")?;
        run(&repo, &["add", "file"])?;
        run(&repo, &["commit", "-m", "initial"])?;
        let main_commit = run(&repo, &["rev-parse", "main"])?;
        run(
            &repo,
            &["remote", "add", "origin", "git@github.com:owner/repo.git"],
        )?;
        run(&repo, &["checkout", "-b", "work"])?;
        fs::write(repo.join("file"), "work")?;
        run(&repo, &["add", "file"])?;
        run(&repo, &["commit", "-m", "work change"])?;
        let issue = RankedIssue {
            issue: crate::model::Issue {
                number: 42,
                title: String::new(),
                body: String::new(),
                source_order: 0,
            },
            score: 1.0,
            prefix: "fix".to_owned(),
        };
        let reference = RepositoryRef {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        check(
            create_branch_at(&root, &reference, &issue)? == "fix/issue-42",
            "branch name mismatch",
        )?;
        check(
            run(&repo, &["branch", "--show-current"])? == "fix/issue-42",
            "new branch was not checked out",
        )?;
        check(
            run(&repo, &["rev-parse", "fix/issue-42"])? == main_commit,
            "branch did not start at main",
        )?;
        check(
            fs::read_to_string(repo.join("file"))? == "main",
            "checked out branch does not contain main's file contents",
        )?;
        let branch_before_duplicate = run(&repo, &["branch", "--list"])?;
        let checkout_before_duplicate = run(&repo, &["branch", "--show-current"])?;
        check(
            create_branch_at(&root, &reference, &issue).is_err(),
            "duplicate branch was accepted",
        )?;
        check(
            run(&repo, &["branch", "--show-current"])? == checkout_before_duplicate,
            "duplicate attempt changed checkout",
        )?;
        check(
            run(&repo, &["branch", "--list"])? == branch_before_duplicate,
            "duplicate attempt changed local branches",
        )?;
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn saves_and_reloads_root_and_requires_repo_directory_to_be_git_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir()?;
        let root = temp.join("projects");
        fs::create_dir(&root)?;
        let config = temp.join("config.json");
        write_config(&config, &root)?;
        check(read_config(&config)? == root, "saved root did not reload")?;

        let repo = root.join("repo");
        fs::create_dir(&repo)?;
        run(&repo, &["init", "--initial-branch=main"])?;
        run(&repo, &["config", "user.email", "test@example.com"])?;
        run(&repo, &["config", "user.name", "Test"])?;
        fs::write(repo.join("file"), "main")?;
        run(&repo, &["add", "file"])?;
        run(&repo, &["commit", "-m", "initial"])?;
        run(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        )?;
        let issue = RankedIssue {
            issue: crate::model::Issue {
                number: 8,
                title: String::new(),
                body: String::new(),
                source_order: 0,
            },
            score: 1.0,
            prefix: "docs".to_owned(),
        };
        let enclosing_repo = temp.join("enclosing");
        fs::create_dir_all(enclosing_repo.join("repo"))?;
        run(&enclosing_repo, &["init", "--initial-branch=main"])?;
        run(
            &enclosing_repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        )?;
        check(
            create_branch_at(
                &enclosing_repo,
                &RepositoryRef {
                    owner: "owner".into(),
                    repo: "repo".into(),
                },
                &issue,
            )
            .is_err(),
            "nested directory was accepted as repository root",
        )?;
        check(
            create_branch_at(
                &root,
                &RepositoryRef {
                    owner: "other".into(),
                    repo: "repo".into(),
                },
                &issue,
            )
            .is_err(),
            "remote mismatch was accepted",
        )?;
        fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[test]
    fn formats_branch_and_accepts_only_github_origin_forms() {
        let issue = RankedIssue {
            issue: crate::model::Issue {
                number: 42,
                title: String::new(),
                body: String::new(),
                source_order: 0,
            },
            score: 1.0,
            prefix: "fix".to_owned(),
        };
        let repository = RepositoryRef {
            owner: "owner".into(),
            repo: "repo".into(),
        };
        assert_eq!(branch_name(&issue), "fix/issue-42");
        assert!(github_remote_matches(
            "git@github.com:owner/repo.git",
            &repository
        ));
        assert!(!github_remote_matches(
            "https://github.com/other/repo.git",
            &repository
        ));
    }
}
