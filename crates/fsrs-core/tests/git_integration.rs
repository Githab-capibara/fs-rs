//! Integration tests of the git layer against real repositories.

use std::path::{Path, PathBuf};
use std::process::Command;

use fsrs_core::git::{GitRepo, UPSTREAM_REMOTE};

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

/// Create a fork that lags behind upstream by exactly one commit.
///
/// The fork is cloned while upstream has a single commit, then upstream gets a
/// second commit, so the fork is genuinely behind.
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

/// Add the second upstream commit.
fn add_second_upstream_commit(upstream: &Path) {
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

#[test]
fn pending_commits_are_exactly_the_upstream_commits_missing_from_main() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_second_upstream_commit(&upstream);

    let repo = GitRepo::open(&fork).unwrap();
    repo.ensure_upstream_remote(upstream.to_str().unwrap())
        .unwrap();
    repo.fetch_upstream("main").unwrap();

    let pending = repo.pending_commits("main").unwrap();
    assert_eq!(pending.len(), 1, "only the second commit is pending");
    assert_eq!(pending[0].summary, "Second upstream commit");
    assert_eq!(pending[0].body, "With a body");
    assert_eq!(pending[0].files, vec!["b.txt".to_string()]);
    assert_eq!(pending[0].insertions, 1);
    assert_eq!(pending[0].deletions, 0);
    assert_eq!(pending[0].short_sha.len(), 8);
}

#[test]
fn pending_commits_are_empty_when_fork_is_up_to_date() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_second_upstream_commit(&upstream);

    // Bring the fork fully up to date.
    git(
        &fork,
        &["fetch", "-q", "origin", "main:refs/remotes/origin/main"],
    );
    git(&fork, &["reset", "--hard", "origin/main"]);

    let repo = GitRepo::open(&fork).unwrap();
    repo.ensure_upstream_remote(upstream.to_str().unwrap())
        .unwrap();
    repo.fetch_upstream("main").unwrap();

    assert!(repo.pending_commits("main").unwrap().is_empty());
}

#[test]
fn upstream_remote_is_created_and_repointed() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);

    let repo = GitRepo::open(&fork).unwrap();
    repo.ensure_upstream_remote("https://example.com/first.git")
        .unwrap();
    repo.ensure_upstream_remote("https://example.com/second.git")
        .unwrap();

    let remote = repo.upstream_url().expect("upstream remote should exist");
    assert_eq!(remote, "https://example.com/second.git");
    assert_eq!(UPSTREAM_REMOTE, "upstream");
}

#[test]
fn branch_lifecycle_create_commit_and_merge() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);

    let repo = GitRepo::open(&fork).unwrap();
    if let Err(err) = repo.create_branch("fsrs/implement/abc", "main") {
        panic!("creating branch failed: {err:#}");
    }
    assert_eq!(repo.current_branch().unwrap(), "fsrs/implement/abc");

    std::fs::write(fork.join("c.txt"), "three\n").unwrap();
    let oid = repo.commit_all("feat: add c").unwrap();
    assert_eq!(oid.len(), 40);

    repo.checkout("main").unwrap();
    assert_eq!(repo.current_branch().unwrap(), "main");

    repo.merge_fast_forward("fsrs/implement/abc").unwrap();
    assert!(fork.join("c.txt").exists());
    let status = repo.status().unwrap();
    assert!(
        status.is_empty(),
        "working tree should be clean: {status:?}"
    );
}

#[test]
fn commit_patch_contains_the_diff() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    add_second_upstream_commit(&upstream);

    let repo = GitRepo::open(&fork).unwrap();
    let patch = repo.commit_patch("HEAD").unwrap();
    assert!(patch.contains("a.txt"), "patch should mention the file");
}
