//! Persistent storage for repositories, pending commits, assessments and runs.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::{Assessment, Complexity, Criticality, Repo, Run, RunEvent, RunState, UpstreamCommit};

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
        self.conn.execute(
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
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM repos WHERE path = ?1",
            params![repo.path.to_string_lossy()],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// List all tracked repositories.
    pub fn list_repos(&self) -> Result<Vec<Repo>> {
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
    pub fn replace_commits(&self, repo_id: i64, commits: &[UpstreamCommit]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM commits WHERE repo_id = ?1", params![repo_id])?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO commits (repo_id, sha, short_sha, summary, body, author, author_email,
                                     authored_at, committed_at, files, insertions, deletions, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                    serde_json::to_string(&commit.files)?,
                    commit.insertions,
                    commit.deletions,
                    commit.url,
                ])?;
            }
        }
        tx.execute(
            "UPDATE repos SET pending_commits = ?2 WHERE id = ?1",
            params![repo_id, commits.len() as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Pending commits of a repository, newest first.
    pub fn list_commits(&self, repo_id: i64) -> Result<Vec<UpstreamCommit>> {
        let mut stmt = self.conn.prepare(
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
                files: serde_json::from_str(&files).unwrap_or_default(),
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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

    /// Record a prompt or an answer of one pipeline agent.
    pub fn insert_run_event(&self, event: &RunEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO run_events (run_id, agent, prompt, output, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event.run_id, event.agent, event.prompt, event.output, event.created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// All events of a run, oldest first.
    pub fn list_run_events(&self, run_id: i64) -> Result<Vec<RunEvent>> {
        let mut stmt = self.conn.prepare(
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
