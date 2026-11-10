//! Integration tests of the catalogue against real repositories.

use std::path::{Path, PathBuf};
use std::process::Command;

use fsrs_core::Repo;
use fsrs_core::catalog::{commit_url, sync_repo, to_upstream_commit};

/// Run a git command inside `dir` and assert it succeeded.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("running git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Create an upstream repository with a single commit on `main`.
fn make_upstream(root: &Path) -> PathBuf {
    let path = root.join("upstream");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "upstream@example.com"]);
    git(&path, &["config", "user.name", "Upstream"]);
    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "First upstream commit"]);
    path
}

/// Create a fork that lags behind upstream.
fn make_fork(root: &Path, upstream: &Path) -> PathBuf {
    let path = root.join("fork");
    git(
        root,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    git(&path, &["config", "user.email", "fork@example.com"]);
    git(&path, &["config", "user.name", "Fork"]);
    path
}

/// Add one more commit to the upstream repository.
fn add_upstream_commit(upstream: &Path) {
    std::fs::write(upstream.join("b.txt"), "two\n").unwrap();
    git(upstream, &["add", "."]);
    git(
        upstream,
        &[
            "commit",
            "-q",
            "-m",
            "Second upstream commit\n\nWith a body",
        ],
    );
}

/// Build the repository row of a fork.
fn repo_row(id: i64, fork: &Path, upstream: &Path) -> Repo {
    Repo {
        id,
        name: "fork".to_string(),
        path: fork.to_path_buf(),
        upstream_url: upstream.to_str().unwrap().to_string(),
        upstream_web: "https://example.com/upstream".to_string(),
        main_branch: "main".to_string(),
        pending_commits: 0,
        last_fetched_at: None,
    }
}

#[test]
fn the_commit_url_points_at_the_upstream_project() {
    assert_eq!(
        commit_url("https://github.com/owner/project", "abc123"),
        "https://github.com/owner/project/commit/abc123"
    );
    assert_eq!(
        commit_url("https://github.com/owner/project/", "abc123"),
        "https://github.com/owner/project/commit/abc123"
    );
    assert_eq!(commit_url("", "abc123"), "");
}

#[test]
fn a_pending_commit_becomes_an_upstream_commit() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_upstream_commit(&upstream);

    let repo = fsrs_core::git::GitRepo::open(&fork).unwrap();
    repo.ensure_upstream_remote(upstream.to_str().unwrap())
        .unwrap();
    repo.fetch_upstream("main").unwrap();
    let pending = repo.pending_commits("main").unwrap();

    let commit = to_upstream_commit(42, pending[0].clone(), "https://example.com/upstream");
    assert_eq!(commit.repo_id, 42);
    assert_eq!(commit.summary, "Second upstream commit");
    assert_eq!(commit.body, "With a body");
    assert_eq!(commit.files, vec!["b.txt".to_string()]);
    assert_eq!(
        commit.url,
        format!("https://example.com/upstream/commit/{}", commit.sha)
    );
}

#[test]
fn synchronising_lists_the_commits_that_are_missing() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_upstream_commit(&upstream);

    let commits = sync_repo(&repo_row(1, &fork, &upstream)).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "Second upstream commit");
    assert_eq!(commits[0].repo_id, 1);
    assert!(
        commits[0]
            .url
            .starts_with("https://example.com/upstream/commit/")
    );
}

#[test]
fn synchronising_a_fork_that_is_up_to_date_finds_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_upstream_commit(&upstream);

    git(
        &fork,
        &["fetch", "-q", "origin", "main:refs/remotes/origin/main"],
    );
    git(&fork, &["reset", "--hard", "origin/main"]);

    let commits = sync_repo(&repo_row(1, &fork, &upstream)).unwrap();
    assert!(commits.is_empty());
}

#[test]
fn synchronising_creates_the_upstream_remote() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_upstream_commit(&upstream);

    sync_repo(&repo_row(1, &fork, &upstream)).unwrap();

    let repo = fsrs_core::git::GitRepo::open(&fork).unwrap();
    assert_eq!(
        repo.upstream_url().as_deref(),
        Some(upstream.to_str().unwrap())
    );
}
