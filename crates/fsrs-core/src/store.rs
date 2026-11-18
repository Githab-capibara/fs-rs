//! Persistent storage for repositories, pending commits, assessments and runs.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::git::CommitStats;
use crate::{Assessment, Complexity, Criticality, Repo, Run, RunEvent, RunState, UpstreamCommit};

/// The SHAs of `commits`, for the `json_each` membership test.
fn shas(commits: &[UpstreamCommit]) -> Vec<&str> {
    commits.iter().map(|commit| commit.sha.as_str()).collect()
}

/// SQLite-backed store of all dashboard state.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (and migrate) the database at `path`.
    ///
    /// Use `:memory:` for an ephemeral store.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // WAL keeps the log in the operating system page cache, so a commit no
        // longer waits for a disk flush; NORMAL is the safe companion of that
        // mode (a power loss can lose the last commits, never corrupt).
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // The dashboard reads the whole table on start; a second writer must
        // wait rather than fail.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // 64MB of page cache and 256MB of memory-mapped reading: the commit
        // list of a fork with a long history then lives in RAM.
        conn.pragma_update(None, "cache_size", -64000)?;
        conn.pragma_update(None, "mmap_size", 268435456)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory store, useful for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Create the schema if it does not exist yet.
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS repos (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                name             TEXT NOT NULL,
                path             TEXT NOT NULL UNIQUE,
                upstream_url     TEXT NOT NULL,
                upstream_web     TEXT NOT NULL,
                main_branch      TEXT NOT NULL,
                pending_commits  INTEGER NOT NULL DEFAULT 0,
                last_fetched_at  INTEGER
            );

            CREATE TABLE IF NOT EXISTS commits (
                repo_id        INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                sha            TEXT NOT NULL,
                short_sha      TEXT NOT NULL,
                summary        TEXT NOT NULL,
                body           TEXT NOT NULL,
                author         TEXT NOT NULL,
                author_email   TEXT NOT NULL,
                authored_at    INTEGER NOT NULL,
                committed_at   INTEGER NOT NULL,
                files          TEXT NOT NULL,
                insertions     INTEGER NOT NULL,
                deletions      INTEGER NOT NULL,
                url            TEXT NOT NULL,
                PRIMARY KEY (repo_id, sha)
            );

            CREATE TABLE IF NOT EXISTS assessments (
                repo_id                INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                sha                    TEXT NOT NULL,
                criticality            TEXT NOT NULL,
                complexity             TEXT NOT NULL,
                criticality_confidence REAL NOT NULL,
                complexity_confidence  REAL NOT NULL,
                needs_review           INTEGER NOT NULL DEFAULT 0,
                rationale              TEXT NOT NULL DEFAULT '',
                model                  TEXT NOT NULL,
                created_at             INTEGER NOT NULL,
                PRIMARY KEY (repo_id, sha)
            );

            CREATE TABLE IF NOT EXISTS runs (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id    INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                sha        TEXT NOT NULL,
                branch     TEXT NOT NULL,
                state      TEXT NOT NULL,
                cycles     INTEGER NOT NULL DEFAULT 0,
                model      TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS run_events (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id     INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                agent      TEXT NOT NULL,
                prompt     TEXT NOT NULL,
                output     TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_commits_repo ON commits(repo_id, committed_at DESC);
            CREATE INDEX IF NOT EXISTS idx_run_events_run ON run_events(run_id, id);
            "#,
        )?;
        Ok(())
    }

    /// Insert or update a repository, returning its identifier.
    pub fn upsert_repo(&self, repo: &Repo) -> Result<i64> {
        // One statement: the identifier comes back with the write.
        let id: i64 = self.conn.query_row(
            r#"
            INSERT INTO repos (name, path, upstream_url, upstream_web, main_branch, pending_commits, last_fetched_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                upstream_url = excluded.upstream_url,
                upstream_web = excluded.upstream_web,
                main_branch = excluded.main_branch,
                pending_commits = excluded.pending_commits,
                last_fetched_at = excluded.last_fetched_at
            RETURNING id
            "#,
            params![
                repo.name,
                repo.path.to_string_lossy(),
                repo.upstream_url,
                repo.upstream_web,
                repo.main_branch,
                repo.pending_commits,
                repo.last_fetched_at,
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// List all tracked repositories.
    pub fn list_repos(&self) -> Result<Vec<Repo>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, path, upstream_url, upstream_web, main_branch, pending_commits, last_fetched_at FROM repos ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Repo {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get::<_, String>(2)?.into(),
                upstream_url: row.get(3)?,
                upstream_web: row.get(4)?,
                main_branch: row.get(5)?,
                pending_commits: row.get(6)?,
                last_fetched_at: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Fetch a repository by identifier.
    pub fn repo(&self, id: i64) -> Result<Option<Repo>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, path, upstream_url, upstream_web, main_branch, pending_commits, last_fetched_at FROM repos WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(Repo {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get::<_, String>(2)?.into(),
            upstream_url: row.get(3)?,
            upstream_web: row.get(4)?,
            main_branch: row.get(5)?,
            pending_commits: row.get(6)?,
            last_fetched_at: row.get(7)?,
        }))
    }

    /// Remove a repository and everything attached to it.
    pub fn remove_repo(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM repos WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Replace the pending commit list of a repository.
    ///
    /// Every commit is upserted by its SHA: the rows of the commits that are
    /// already stored are updated in place instead of being rewritten. The
    /// whole list is still replaced as a set — the SHAs that are absent from
    /// `commits` are deleted.
    ///
    /// A call whose list is already stored writes nothing at all: a poll of a
    /// repository that did not move costs one read of the SHAs and no page.
    /// A commit is immutable, so a stored row of a pending SHA is already
    /// correct and rewriting it would only burn a page.
    pub fn replace_commits(&self, repo_id: i64, commits: &[UpstreamCommit]) -> Result<()> {
        if self.commits_match(repo_id, commits)? {
            // The pending count is still confirmed: it is the number of rows
            // that the list matched.
            self.conn.execute(
                "UPDATE repos SET pending_commits = ?2 WHERE id = ?1",
                params![repo_id, commits.len() as i64],
            )?;
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                r#"
                INSERT INTO commits (repo_id, sha, short_sha, summary, body, author, author_email,
                                     authored_at, committed_at, files, insertions, deletions, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(repo_id, sha) DO UPDATE SET
                    short_sha = excluded.short_sha,
                    summary = excluded.summary,
                    body = excluded.body,
                    author = excluded.author,
                    author_email = excluded.author_email,
                    authored_at = excluded.authored_at,
                    committed_at = excluded.committed_at,
                    files = excluded.files,
                    insertions = excluded.insertions,
                    deletions = excluded.deletions,
                    url = excluded.url
                "#,
            )?;
            for commit in commits {
                stmt.execute(params![
                    repo_id,
                    commit.sha,
                    commit.short_sha,
                    commit.summary,
                    commit.body,
                    commit.author,
                    commit.author_email,
                    commit.authored_at,
                    commit.committed_at,
                    sonic_rs::to_string(&commit.files)?,
                    commit.insertions,
                    commit.deletions,
                    commit.url,
                ])?;
            }
        }
        // Keep the set semantics of the previous implementation: the commits
        // that are no longer pending leave the store.
        let mut delete = tx.prepare_cached(
            "DELETE FROM commits WHERE repo_id = ?1 AND sha NOT IN (SELECT value FROM json_each(?2))",
        )?;
        delete.execute(params![repo_id, sonic_rs::to_string(&shas(commits))?])?;
        drop(delete);
        tx.execute(
            "UPDATE repos SET pending_commits = ?2 WHERE id = ?1",
            params![repo_id, commits.len() as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Whether the store already holds exactly the SHAs of `commits`.
    ///
    /// One statement reads the SHAs of the repository; the comparison happens
    /// against the list, which is short. A difference of either side returns
    /// false, so the caller falls back to writing.
    fn commits_match(&self, repo_id: i64, commits: &[UpstreamCommit]) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT sha FROM commits WHERE repo_id = ?1")?;
        let stored: std::collections::HashSet<String> = stmt
            .query_map(params![repo_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<std::collections::HashSet<String>, _>>()?;
        if stored.len() != commits.len() {
            return Ok(false);
        }
        Ok(commits.iter().all(|commit| stored.contains(&commit.sha)))
    }

    /// Fetch the commits of a repository, newest first.
    ///
    /// Used to reuse the diff statistics of a previous synchronisation.
    pub fn commit_stats(&self, repo_id: i64) -> Result<HashMap<String, CommitStats>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT sha, files, insertions, deletions FROM commits WHERE repo_id = ?1",
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            let files: String = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                CommitStats {
                    files: sonic_rs::from_str::<Vec<String>>(&files).unwrap_or_default(),
                    insertions: row.get(2)?,
                    deletions: row.get(3)?,
                },
            ))
        })?;
        let mut out = HashMap::with_capacity(1024);
        for row in rows {
            let (sha, stats) = row?;
            out.insert(sha, stats);
        }
        Ok(out)
    }

    /// Pending commits of a repository, newest first.
    pub fn list_commits(&self, repo_id: i64) -> Result<Vec<UpstreamCommit>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT repo_id, sha, short_sha, summary, body, author, author_email,
                   authored_at, committed_at, files, insertions, deletions, url
            FROM commits WHERE repo_id = ?1 ORDER BY committed_at DESC
            "#,
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            let files: String = row.get(9)?;
            Ok(UpstreamCommit {
                repo_id: row.get(0)?,
                sha: row.get(1)?,
                short_sha: row.get(2)?,
                summary: row.get(3)?,
                body: row.get(4)?,
                author: row.get(5)?,
                author_email: row.get(6)?,
                authored_at: row.get(7)?,
                committed_at: row.get(8)?,
                files: sonic_rs::from_str::<Vec<String>>(&files).unwrap_or_default(),
                insertions: row.get(10)?,
                deletions: row.get(11)?,
                url: row.get(12)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Store an assessment, replacing any previous one for the same commit.
    pub fn upsert_assessment(&self, assessment: &Assessment) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO assessments (repo_id, sha, criticality, complexity, criticality_confidence,
                                     complexity_confidence, needs_review, rationale, model, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(repo_id, sha) DO UPDATE SET
                criticality = excluded.criticality,
                complexity = excluded.complexity,
                criticality_confidence = excluded.criticality_confidence,
                complexity_confidence = excluded.complexity_confidence,
                needs_review = excluded.needs_review,
                rationale = excluded.rationale,
                model = excluded.model,
                created_at = excluded.created_at
            "#,
            params![
                assessment.repo_id,
                assessment.sha,
                assessment.criticality.as_str(),
                assessment.complexity.as_str(),
                assessment.criticality_confidence,
                assessment.complexity_confidence,
                assessment.needs_review as i64,
                assessment.rationale,
                assessment.model,
                assessment.created_at,
            ],
        )?;
        Ok(())
    }

    /// Fetch the assessment of one commit.
    pub fn assessment(&self, repo_id: i64, sha: &str) -> Result<Option<Assessment>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT repo_id, sha, criticality, complexity, criticality_confidence,
                   complexity_confidence, needs_review, rationale, model, created_at
            FROM assessments WHERE repo_id = ?1 AND sha = ?2
            "#,
        )?;
        let mut rows = stmt.query(params![repo_id, sha])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let criticality: String = row.get(2)?;
        let complexity: String = row.get(3)?;
        Ok(Some(Assessment {
            repo_id: row.get(0)?,
            sha: row.get(1)?,
            criticality: Criticality::parse(&criticality).unwrap_or(Criticality::Medium),
            complexity: Complexity::parse(&complexity).unwrap_or(Complexity::Medium),
            criticality_confidence: row.get(4)?,
            complexity_confidence: row.get(5)?,
            needs_review: row.get::<_, i64>(6)? != 0,
            rationale: row.get(7)?,
            model: row.get(8)?,
            created_at: row.get(9)?,
        }))
    }

    /// Fetch every assessment of a repository.
    pub fn list_assessments(&self, repo_id: i64) -> Result<Vec<Assessment>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT repo_id, sha, criticality, complexity, criticality_confidence,
                   complexity_confidence, needs_review, rationale, model, created_at
            FROM assessments WHERE repo_id = ?1
            "#,
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            let criticality: String = row.get(2)?;
            let complexity: String = row.get(3)?;
            Ok(Assessment {
                repo_id: row.get(0)?,
                sha: row.get(1)?,
                criticality: Criticality::parse(&criticality).unwrap_or(Criticality::Medium),
                complexity: Complexity::parse(&complexity).unwrap_or(Complexity::Medium),
                criticality_confidence: row.get(4)?,
                complexity_confidence: row.get(5)?,
                needs_review: row.get::<_, i64>(6)? != 0,
                rationale: row.get(7)?,
                model: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Insert a run, returning its identifier.
    pub fn insert_run(&self, run: &Run) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO runs (repo_id, sha, branch, state, cycles, model, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                run.repo_id,
                run.sha,
                run.branch,
                run.state.as_str(),
                run.cycles,
                run.model,
                run.created_at,
                run.updated_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update the state of a run.
    pub fn update_run_state(&self, id: i64, state: RunState) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, state.as_str(), now_unix()],
        )?;
        Ok(())
    }

    /// Increment the cycle counter of a run.
    pub fn bump_run_cycles(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET cycles = cycles + 1, updated_at = ?2 WHERE id = ?1",
            params![id, now_unix()],
        )?;
        Ok(())
    }

    /// Fetch a run by identifier.
    pub fn run(&self, id: i64) -> Result<Option<Run>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, repo_id, sha, branch, state, cycles, model, created_at, updated_at FROM runs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let state: String = row.get(4)?;
        Ok(Some(Run {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            sha: row.get(2)?,
            branch: row.get(3)?,
            state: RunState::parse(&state).unwrap_or(RunState::Failed),
            cycles: row.get(5)?,
            model: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        }))
    }

    /// List all runs of a repository, newest first.
    pub fn list_runs(&self, repo_id: i64) -> Result<Vec<Run>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, repo_id, sha, branch, state, cycles, model, created_at, updated_at FROM runs WHERE repo_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            let state: String = row.get(4)?;
            Ok(Run {
                id: row.get(0)?,
                repo_id: row.get(1)?,
                sha: row.get(2)?,
                branch: row.get(3)?,
                state: RunState::parse(&state).unwrap_or(RunState::Failed),
                cycles: row.get(5)?,
                model: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Record one prompt or answer of a pipeline agent.
    pub fn insert_run_event(&self, event: &RunEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO run_events (run_id, agent, prompt, output, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event.run_id, event.agent, event.prompt, event.output, event.created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Record a batch of pipeline events inside one transaction.
    ///
    /// The events of a pipeline arrive one by one, but a flush of the queue in
    /// the interface handles several of them at once: one transaction for the
    /// batch instead of one commit per event.
    pub fn insert_run_events(&self, events: &[RunEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO run_events (run_id, agent, prompt, output, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for event in events {
                stmt.execute(params![
                    event.run_id,
                    event.agent,
                    event.prompt,
                    event.output,
                    event.created_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// All events of a run, oldest first.
    pub fn list_run_events(&self, run_id: i64) -> Result<Vec<RunEvent>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT run_id, agent, prompt, output, created_at FROM run_events WHERE run_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(RunEvent {
                run_id: row.get(0)?,
                agent: row.get(1)?,
                prompt: row.get(2)?,
                output: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// Current unix timestamp in seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repo;

    fn repo(name: &str) -> Repo {
        Repo {
            id: 0,
            name: name.to_string(),
            path: std::path::PathBuf::from(format!("/tmp/{name}")),
            upstream_url: "https://example.com/upstream.git".to_string(),
            upstream_web: "https://example.com/upstream".to_string(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        }
    }

    #[test]
    fn upsert_repo_is_idempotent_by_path() {
        let store = Store::open_in_memory().unwrap();
        let first = store.upsert_repo(&repo("a")).unwrap();
        let second = store.upsert_repo(&repo("a")).unwrap();
        assert_eq!(first, second);
        assert_eq!(store.list_repos().unwrap().len(), 1);
    }

    #[test]
    fn commits_are_stored_and_replaced() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let commit = UpstreamCommit {
            repo_id,
            sha: "abc123".into(),
            short_sha: "abc12345".into(),
            summary: "Fix".into(),
            body: String::new(),
            author: "dev".into(),
            author_email: "dev@example.com".into(),
            authored_at: 1,
            committed_at: 2,
            files: vec!["src/lib.rs".into()],
            insertions: 3,
            deletions: 1,
            url: "https://example.com/commit/abc123".into(),
        };
        store.replace_commits(repo_id, &[commit]).unwrap();
        let commits = store.list_commits(repo_id).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].files, vec!["src/lib.rs".to_string()]);
        assert_eq!(store.list_repos().unwrap()[0].pending_commits, 1);

        store.replace_commits(repo_id, &[]).unwrap();
        assert!(store.list_commits(repo_id).unwrap().is_empty());
        assert_eq!(store.list_repos().unwrap()[0].pending_commits, 0);
    }

    #[test]
    fn replacing_commits_upserts_by_sha_and_drops_the_rest() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let first = UpstreamCommit {
            repo_id,
            sha: "sha-1".into(),
            short_sha: "sha-1111".into(),
            summary: "First".into(),
            body: String::new(),
            author: "dev".into(),
            author_email: "dev@example.com".into(),
            authored_at: 10,
            committed_at: 10,
            files: vec!["a.rs".into(), "b.rs".into()],
            insertions: 5,
            deletions: 2,
            url: "https://example.com/commit/sha-1".into(),
        };
        let second = UpstreamCommit {
            sha: "sha-2".into(),
            short_sha: "sha-2222".into(),
            summary: "Second".into(),
            url: "https://example.com/commit/sha-2".into(),
            ..first.clone()
        };
        store
            .replace_commits(repo_id, &[first.clone(), second.clone()])
            .unwrap();
        assert_eq!(store.list_commits(repo_id).unwrap().len(), 2);

        // The fork moved: the statistics of sha-1 were updated and sha-2 left.
        let updated = UpstreamCommit {
            insertions: 50,
            deletions: 21,
            ..first.clone()
        };
        store.replace_commits(repo_id, &[updated]).unwrap();
        let stored = store.list_commits(repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].sha, "sha-1");
        assert_eq!(stored[0].insertions, 50);
        assert_eq!(stored[0].deletions, 21);

        // The statistics are served back keyed by SHA, for the incremental
        // listing of the git layer.
        let stats = store.commit_stats(repo_id).unwrap();
        let cached = stats.get("sha-1").expect("sha-1 statistics are cached");
        assert_eq!(cached.files, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(cached.insertions, 50);
        assert_eq!(cached.deletions, 21);
        assert_eq!(store.commit_stats(9999).unwrap().len(), 0);
    }

    #[test]
    fn the_same_list_is_written_once() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let commit = UpstreamCommit {
            repo_id,
            sha: "sha-1".into(),
            short_sha: "sha-1111".into(),
            summary: "First".into(),
            body: String::new(),
            author: "dev".into(),
            author_email: "dev@example.com".into(),
            authored_at: 1,
            committed_at: 1,
            files: vec!["a.rs".into()],
            insertions: 1,
            deletions: 1,
            url: "https://example.com/commit/sha-1".into(),
        };
        store
            .replace_commits(repo_id, std::slice::from_ref(&commit))
            .unwrap();

        // A poll of a repository that did not move writes nothing: the row of
        // a pending commit is immutable, so it is already correct.
        store
            .replace_commits(repo_id, std::slice::from_ref(&commit))
            .unwrap();
        store.replace_commits(repo_id, &[commit]).unwrap();
        let stored = store.list_commits(repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].summary, "First");
        assert_eq!(store.list_repos().unwrap()[0].pending_commits, 1);

        // One more commit pending: the list changed, so the store writes.
        let other = UpstreamCommit {
            sha: "sha-2".into(),
            ..stored[0].clone()
        };
        store
            .replace_commits(repo_id, &[other, stored[0].clone()])
            .unwrap();
        assert_eq!(store.list_commits(repo_id).unwrap().len(), 2);

        // One commit merged into the fork: the set shrinks, so the store
        // writes the row away.
        store
            .replace_commits(repo_id, &[stored[0].clone()])
            .unwrap();
        let stored = store.list_commits(repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].sha, "sha-1");
    }

    #[test]
    fn run_events_are_written_in_one_batch() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let run_id = store
            .insert_run(&Run {
                id: 0,
                repo_id,
                sha: "sha-1".into(),
                branch: "fsrs/implement/sha-1".into(),
                state: RunState::Prepared,
                cycles: 0,
                model: "opencode/gpt-5".into(),
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let events = vec![
            RunEvent {
                run_id,
                agent: "implementer".into(),
                prompt: "one".into(),
                output: "out-one".into(),
                created_at: 2,
            },
            RunEvent {
                run_id,
                agent: "verifier".into(),
                prompt: "two".into(),
                output: "out-two".into(),
                created_at: 3,
            },
        ];
        store.insert_run_events(&events).unwrap();
        store.insert_run_events(&[]).unwrap();
        let loaded = store.list_run_events(run_id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].prompt, "one");
        assert_eq!(loaded[1].agent, "verifier");
    }

    #[test]
    fn assessments_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let assessment = Assessment {
            repo_id,
            sha: "abc".into(),
            criticality: Criticality::Critical,
            complexity: Complexity::Hard,
            criticality_confidence: 0.91,
            complexity_confidence: 0.72,
            needs_review: true,
            rationale: "because".into(),
            model: "foq".into(),
            created_at: 42,
        };
        store.upsert_assessment(&assessment).unwrap();
        let loaded = store.assessment(repo_id, "abc").unwrap().unwrap();
        assert_eq!(loaded.criticality, Criticality::Critical);
        assert_eq!(loaded.complexity, Complexity::Hard);
        assert!(loaded.needs_review);
        assert_eq!(loaded.rationale, "because");
    }

    #[test]
    fn runs_and_events_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let repo_id = store.upsert_repo(&repo("a")).unwrap();
        let run = Run {
            id: 0,
            repo_id,
            sha: "abc".into(),
            branch: "fsrs/implement/abc".into(),
            state: RunState::Prepared,
            cycles: 0,
            model: "opencode/gpt-5".into(),
            created_at: 1,
            updated_at: 1,
        };
        let run_id = store.insert_run(&run).unwrap();
        store.update_run_state(run_id, RunState::Reviewing).unwrap();
        store.bump_run_cycles(run_id).unwrap();
        let loaded = store.run(run_id).unwrap().unwrap();
        assert_eq!(loaded.state, RunState::Reviewing);
        assert_eq!(loaded.cycles, 1);

        store
            .insert_run_event(&RunEvent {
                run_id,
                agent: "implementer".into(),
                prompt: "do it".into(),
                output: "done".into(),
                created_at: 3,
            })
            .unwrap();
        let events = store.list_run_events(run_id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].prompt, "do it");
    }
}
