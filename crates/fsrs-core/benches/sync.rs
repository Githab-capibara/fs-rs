//! Benchmark of the synchronisation of a fork with its upstream.
//!
//! The two operations of the dashboard that a user waits for are measured:
//!
//! * listing the missing commits of the fork, which used to pay a tree-to-tree
//!   diff for every commit plus a graph walk for every commit found,
//! * storing the list in SQLite, which used to delete every row and insert
//!   them again.
//!
//! The fixture is a real upstream with 1500 commits — the order of magnitude
//! of a project like `fd`. The fork is `BEHIND` commits back, so the listing
//! walks the backlog a user actually comes back to.
//!
//! ```bash
//! cargo bench -p fsrs-core
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use fsrs_core::catalog::sync_repo;
use fsrs_core::git::{CommitStats, GitRepo};
use fsrs_core::{Repo, Store, UpstreamCommit};
use rusqlite::params;

/// How many commits the upstream of the fixture carries.
const UPSTREAM_COMMITS: usize = 1500;

/// How many commits the fork of the fixture is behind its upstream.
const BEHIND: usize = 200;

/// The rows the storage benchmark stores.
const STORED_COMMITS: usize = 500;

/// Run a git command inside `dir` and return its standard output.
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

/// Create the upstream of the fixture with `UPSTREAM_COMMITS` commits.
///
/// The commits each change one small file, so every diff is real and small:
/// the measurement cannot be won by an artificial diff-free history.
fn make_upstream(root: &Path) -> PathBuf {
    let path = root.join("upstream");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "upstream@example.com"]);
    git(&path, &["config", "user.name", "Upstream"]);
    for step in 0..UPSTREAM_COMMITS {
        std::fs::write(path.join("a.txt"), format!("{step}\n")).unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-q", "-m", &format!("change {step}")]);
    }
    path
}

/// Clone the upstream and reset the fork `BEHIND` commits back.
fn make_fork(root: &Path, upstream: &Path) -> PathBuf {
    // Clone the full history, then reset to a commit BEHIND steps back: a
    // shallow clone cannot fetch more later, and the whole point is a fork
    // that lags by a real backlog.
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
    git(&path, &["reset", "-q", "--hard", &format!("HEAD~{BEHIND}")]);
    path
}

/// Fetch the upstream branch, so the objects of the fork are complete.
fn prepare(git: &GitRepo, upstream: &Path) {
    git.ensure_upstream_remote(upstream.to_str().unwrap())
        .unwrap();
    git.fetch_upstream("main").unwrap();
}

/// A repository row of the fork.
fn repo_row(id: i64, fork: &Path, upstream: &Path) -> Repo {
    Repo {
        id,
        name: "fork".into(),
        path: fork.to_path_buf(),
        upstream_url: upstream.to_str().unwrap().to_string(),
        upstream_web: "https://example.com/upstream".into(),
        main_branch: "main".into(),
        pending_commits: 0,
        last_fetched_at: None,
    }
}

/// The listing of the missing commits, with and without the known statistics.
fn bench_pending(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    let git = GitRepo::open(&fork).unwrap();
    prepare(&git, &upstream);

    let pending = git.pending_commits("main").unwrap();
    assert_eq!(pending.len(), BEHIND);

    // The statistics of the first listing are enough for every later one.
    let known: HashMap<String, CommitStats> = pending
        .iter()
        .map(|commit| {
            (
                commit.sha.clone(),
                CommitStats {
                    files: commit.files.clone(),
                    insertions: commit.insertions,
                    deletions: commit.deletions,
                },
            )
        })
        .collect();

    let mut group = c.benchmark_group(format!("pending_commits ({BEHIND} pending)"));
    group.sample_size(10);
    group.bench_function("full", |b| {
        b.iter(|| {
            let commits = git.pending_commits("main").unwrap();
            criterion::black_box(commits.len());
        })
    });
    group.bench_function("with known statistics", |b| {
        b.iter(|| {
            let commits = git.pending_commits_incremental("main", &known).unwrap();
            criterion::black_box(commits.len());
        })
    });
    group.bench_function("first listing, empty statistics", |b| {
        b.iter(|| {
            let commits = git
                .pending_commits_incremental("main", &HashMap::new())
                .unwrap();
            criterion::black_box(commits.len());
        })
    });
    group.bench_function("metadata only, no diff", |b| {
        b.iter(|| {
            let commits = git.pending_commit_metadata("main").unwrap();
            criterion::black_box(commits.len());
        })
    });
    group.finish();
}

/// The cost of the statistics of one commit.
///
/// The statistics are the expensive half of a commit: two trees are compared
/// for every commit. The measurement separates them from the cheap half, the
/// metadata, so the price of the two halves is known.
fn bench_commit_stats(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    let git = GitRepo::open(&fork).unwrap();
    prepare(&git, &upstream);

    let pending = git.pending_commits("main").unwrap();
    assert_eq!(pending.len(), BEHIND);
    // The commit whose statistics are measured again and again: the first of
    // the pending ones.
    let sha = pending[0].sha.clone();
    let commit = git.resolve_commit(&sha).unwrap();

    let mut group = c.benchmark_group("commit_stats");
    group.sample_size(10);
    group.bench_function("statistics of one commit", |b| {
        b.iter(|| {
            let stats = git.commit_stats(&commit).unwrap();
            criterion::black_box(stats.insertions);
        })
    });
    group.bench_function("metadata of one commit", |b| {
        b.iter(|| {
            let meta = git.commit_meta(&commit, None).unwrap();
            criterion::black_box(meta.sha.len());
        })
    });
    group.finish();
}

/// Storing a long pending list in SQLite.
fn bench_replace(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);

    let commit = |repo_id: i64, sha: &str| UpstreamCommit {
        repo_id,
        sha: sha.to_string(),
        short_sha: sha[..8].to_string(),
        summary: format!("change {sha}"),
        body: String::new(),
        author: "upstream".into(),
        author_email: "upstream@example.com".into(),
        authored_at: 1_700_000_000,
        committed_at: 1_700_000_000,
        files: vec!["a.txt".to_string()],
        insertions: 1,
        deletions: 1,
        url: format!("https://example.com/commit/{sha}"),
    };
    let commits: Vec<UpstreamCommit> = (0..STORED_COMMITS)
        .map(|index| commit(1, &format!("{index:040x}")))
        .collect();

    // A store on disk, exactly like the dashboard opens it: WAL keeps the log
    // in the page cache, so a commit does not wait for a disk flush while the
    // cost of writing a page is real — that is what the user waits for.
    let database = temp.path().join("fsrs.db");
    let store = Store::open(&database).unwrap();
    let repo_id = store.upsert_repo(&repo_row(1, &fork, &upstream)).unwrap();
    store.replace_commits(repo_id, &commits).unwrap();

    // The schema of the store, for the raw comparison below.
    let schema = "CREATE TABLE commits (
            repo_id INTEGER NOT NULL,
            sha TEXT NOT NULL,
            short_sha TEXT, summary TEXT, body TEXT, author TEXT, author_email TEXT,
            authored_at INTEGER, committed_at INTEGER, files TEXT,
            insertions INTEGER, deletions INTEGER, url TEXT,
            PRIMARY KEY (repo_id, sha))";
    let raw = rusqlite::Connection::open(temp.path().join("raw.db")).unwrap();
    raw.pragma_update(None, "journal_mode", "WAL").unwrap();
    raw.pragma_update(None, "synchronous", "NORMAL").unwrap();
    raw.execute_batch(schema).unwrap();
    for commit in &commits {
        raw.execute(
            "INSERT INTO commits (repo_id, sha, short_sha, summary, body, author, author_email,
                                  authored_at, committed_at, files, insertions, deletions, url)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                commit.sha,
                commit.short_sha,
                commit.summary,
                commit.body,
                commit.author,
                commit.author_email,
                commit.authored_at,
                commit.committed_at,
                sonic_rs::to_string(&commit.files).unwrap(),
                commit.insertions,
                commit.deletions,
                commit.url,
            ],
        )
        .unwrap();
    }

    let mut group = c.benchmark_group(format!("replace_commits ({STORED_COMMITS} rows)"));
    group.sample_size(10);

    // Same rows again: the upsert has to detect that nothing changed.
    group.bench_function("upsert, nothing changed", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for _ in 0..iterations {
                store.replace_commits(repo_id, &commits).unwrap();
            }
            started.elapsed()
        })
    });

    // Every row carries a new summary, so every row is rewritten either way.
    group.bench_function("upsert, every row changed", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for iteration in 0..iterations {
                let rows: Vec<UpstreamCommit> = commits
                    .iter()
                    .map(|commit| UpstreamCommit {
                        summary: format!("iteration {iteration}"),
                        ..commit.clone()
                    })
                    .collect();
                store.replace_commits(repo_id, &rows).unwrap();
            }
            started.elapsed()
        })
    });

    // The path the store used before: every row deleted, every row inserted.
    group.bench_function("delete all + insert", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for iteration in 0..iterations {
                let tx = raw.unchecked_transaction().unwrap();
                {
                    let mut delete = tx
                        .prepare_cached("DELETE FROM commits WHERE repo_id = 1")
                        .unwrap();
                    delete.execute(params![]).unwrap();
                }
                for commit in &commits {
                    let mut insert = tx
                        .prepare_cached(
                            "INSERT INTO commits (repo_id, sha, short_sha, summary, body, author,
                                                  author_email, authored_at, committed_at, files,
                                                  insertions, deletions, url)
                             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        )
                        .unwrap();
                    insert
                        .execute(params![
                            commit.sha,
                            commit.short_sha,
                            format!("iteration {iteration}"),
                            commit.body,
                            commit.author,
                            commit.author_email,
                            commit.authored_at,
                            commit.committed_at,
                            sonic_rs::to_string(&commit.files).unwrap(),
                            commit.insertions,
                            commit.deletions,
                            commit.url,
                        ])
                        .unwrap();
                }
                tx.commit().unwrap();
            }
            started.elapsed()
        })
    });
    group.finish();
}

/// The end-to-end synchronisation of a fork.
fn bench_sync_end_to_end(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    let repo = repo_row(1, &fork, &upstream);
    let store = Store::open(&temp.path().join("fsrs.db")).unwrap();
    let repo_id = store.upsert_repo(&repo).unwrap();

    let commits = sync_repo(&repo, &Default::default()).unwrap();
    assert_eq!(commits.len(), BEHIND);
    store.replace_commits(repo_id, &commits).unwrap();
    let known = store.commit_stats(repo_id).unwrap();

    let mut group = c.benchmark_group(format!("sync_repo ({BEHIND} pending)"));
    group.sample_size(10);
    group.bench_function("cold, nothing known", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for _ in 0..iterations {
                let commits = sync_repo(&repo, &Default::default()).unwrap();
                criterion::black_box(commits.len());
            }
            started.elapsed()
        })
    });
    group.bench_function("warm, the statistics are known", |b| {
        b.iter_custom(|iterations| {
            let started = Instant::now();
            for _ in 0..iterations {
                let commits = sync_repo(&repo, &known).unwrap();
                criterion::black_box(commits.len());
            }
            started.elapsed()
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_pending,
    bench_commit_stats,
    bench_replace,
    bench_sync_end_to_end
);
criterion_main!(benches);
