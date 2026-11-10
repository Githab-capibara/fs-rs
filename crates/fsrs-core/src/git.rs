//! Git operations against tracked fork repositories.
//!
//! Every operation works on the local working copy of a fork. The upstream
//! remote is fetched into `refs/remotes/upstream/<branch>` and pending commits
//! are the ones reachable from upstream but not from the fork's main branch.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use git2::{Commit, DiffOptions, FetchOptions, Repository};

/// Name of the remote that points at the original upstream project.
pub const UPSTREAM_REMOTE: &str = "upstream";

/// A commit of the upstream project that is not yet present in the fork.
#[derive(Debug, Clone)]
pub struct PendingCommit {
    /// Full object id.
    pub sha: String,
    /// Short object id.
    pub short_sha: String,
    /// First line of the message.
    pub summary: String,
    /// Remaining message body.
    pub body: String,
    /// Author name.
    pub author: String,
    /// Author email.
    pub author_email: String,
    /// Author timestamp, unix seconds.
    pub authored_at: i64,
    /// Committer timestamp, unix seconds.
    pub committed_at: i64,
    /// Files touched by the commit.
    pub files: Vec<String>,
    /// Inserted line count.
    pub insertions: u32,
    /// Deleted line count.
    pub deletions: u32,
}

/// Diff statistics of a single commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitStats {
    /// Files touched.
    pub files: Vec<String>,
    /// Inserted lines.
    pub insertions: u32,
    /// Deleted lines.
    pub deletions: u32,
}

/// A tracked repository opened from disk.
pub struct GitRepo {
    repo: Repository,
}

impl GitRepo {
    /// Open the repository at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path)
            .with_context(|| format!("opening git repository at {}", path.display()))?;
        Ok(Self { repo })
    }

    /// Path of the working copy.
    pub fn workdir(&self) -> Option<&Path> {
        self.repo.workdir()
    }

    /// Current branch name, `HEAD` when detached.
    pub fn current_branch(&self) -> Result<String> {
        let head = self.repo.head().context("reading HEAD")?;
        Ok(head.shorthand().unwrap_or("HEAD").to_string())
    }

    /// Resolve a branch to a commit.
    pub fn branch_commit(&self, branch: &str) -> Result<Commit<'_>> {
        let branch_ref = self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .with_context(|| format!("finding local branch {branch}"))?;
        branch_ref
            .get()
            .peel_to_commit()
            .with_context(|| format!("peeling branch {branch} to a commit"))
    }

    /// URL of the `upstream` remote, when configured.
    pub fn upstream_url(&self) -> Option<String> {
        self.repo
            .find_remote(UPSTREAM_REMOTE)
            .ok()
            .and_then(|remote| remote.url().ok().map(|url| url.to_string()))
    }

    /// Ensure the `upstream` remote exists and points at `url`.
    pub fn ensure_upstream_remote(&self, url: &str) -> Result<()> {
        if let Ok(remote) = self.repo.find_remote(UPSTREAM_REMOTE) {
            if remote.url().ok() != Some(url) {
                self.repo
                    .remote_set_url(UPSTREAM_REMOTE, url)
                    .with_context(|| format!("updating upstream remote to {url}"))?;
            }
            return Ok(());
        }
        self.repo
            .remote(UPSTREAM_REMOTE, url)
            .with_context(|| format!("creating upstream remote for {url}"))?;
        Ok(())
    }

    /// Fetch the upstream remote for `branch` into
    /// `refs/remotes/upstream/<branch>`.
    pub fn fetch_upstream(&self, branch: &str) -> Result<()> {
        let mut remote = self
            .repo
            .find_remote(UPSTREAM_REMOTE)
            .context("upstream remote is not configured")?;
        let refspec = format!("+refs/heads/{branch}:refs/remotes/{UPSTREAM_REMOTE}/{branch}");
        let mut options = FetchOptions::new();
        options.download_tags(git2::AutotagOption::All);
        remote
            .fetch(&[refspec.as_str()], Some(&mut options), None)
            .with_context(|| format!("fetching upstream branch {branch}"))?;
        Ok(())
    }

    /// Resolve the remote-tracking ref of the upstream branch.
    pub fn upstream_commit(&self, branch: &str) -> Result<Commit<'_>> {
        let reference = format!("refs/remotes/{UPSTREAM_REMOTE}/{branch}");
        let commit = self
            .repo
            .find_reference(&reference)
            .with_context(|| format!("resolving {reference}; run a fetch first"))?
            .peel_to_commit()?;
        Ok(commit)
    }

    /// Commits reachable from upstream but absent from the fork's `branch`.
    ///
    /// Result is ordered newest first.
    pub fn pending_commits(&self, branch: &str) -> Result<Vec<PendingCommit>> {
        let local = self.branch_commit(branch)?;
        let upstream = self.upstream_commit(branch)?;
        let local_oid = local.id();
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(git2::Sort::TIME)?;
        walk.push(upstream.id())?;
        walk.hide(local_oid)?;
        let mut pending = Vec::new();
        for oid in walk {
            let oid = oid?;
            // Skip commits that are already reachable from the fork's branch.
            if self
                .repo
                .graph_descendant_of(local_oid, oid)
                .unwrap_or(false)
            {
                continue;
            }
            let commit = self.repo.find_commit(oid)?;
            pending.push(self.describe_commit(&commit)?);
        }
        Ok(pending)
    }

    /// Build a [`PendingCommit`] description from a git commit.
    pub fn describe_commit(&self, commit: &Commit<'_>) -> Result<PendingCommit> {
        let message = commit.message().unwrap_or_default();
        let (summary, body) = split_message(message);
        let author = commit.author();
        let stats = self.commit_stats(commit)?;
        Ok(PendingCommit {
            sha: commit.id().to_string(),
            short_sha: commit.id().to_string()[..8].to_string(),
            summary,
            body,
            author: author.name().unwrap_or("unknown").to_string(),
            author_email: author.email().unwrap_or("").to_string(),
            authored_at: author.when().seconds(),
            committed_at: commit.committer().when().seconds(),
            files: stats.files,
            insertions: stats.insertions,
            deletions: stats.deletions,
        })
    }

    /// Compute diff statistics for a commit against its first parent.
    pub fn commit_stats(&self, commit: &Commit<'_>) -> Result<CommitStats> {
        let tree = commit.tree()?;
        let parent_tree = match commit.parent(0) {
            Ok(parent) => Some(parent.tree()?),
            Err(_) => None,
        };
        let mut options = DiffOptions::new();
        options.include_typechange(true);
        let diff =
            self.repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
        let mut stats = CommitStats::default();
        let mut files: BTreeSet<String> = BTreeSet::new();
        diff.foreach(
            &mut |delta, _| {
                if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
                    files.insert(path.to_string_lossy().to_string());
                }
                true
            },
            None,
            None,
            None,
        )?;
        let diff_stats = diff.stats().context("computing diff statistics")?;
        stats.insertions = diff_stats.insertions() as u32;
        stats.deletions = diff_stats.deletions() as u32;
        stats.files = files.into_iter().collect();
        Ok(stats)
    }

    /// Resolve any git revision expression to a commit.
    ///
    /// Accepts full and abbreviated object ids, branch names, `HEAD`, and
    /// expressions such as `HEAD~1` or `main^`.
    pub fn resolve_commit(&self, rev: &str) -> Result<Commit<'_>> {
        let rev = rev.trim();
        if rev.is_empty() {
            return Err(anyhow!("empty revision"));
        }
        let object = self
            .repo
            .revparse_single(rev)
            .with_context(|| format!("resolving revision {rev}"))?;
        object
            .peel_to_commit()
            .with_context(|| format!("revision {rev} is not a commit"))
    }

    /// Raw patch text of a commit, used to feed the agents.
    pub fn commit_patch(&self, rev: &str) -> Result<String> {
        let commit = self.resolve_commit(rev)?;
        let tree = commit.tree()?;
        let parent_tree = match commit.parent(0) {
            Ok(parent) => Some(parent.tree()?),
            Err(_) => None,
        };
        let mut options = DiffOptions::new();
        options.include_typechange(true);
        let diff =
            self.repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
        let mut out = String::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            match line.origin() {
                '+' | '-' | ' ' => out.push(line.origin()),
                _ => {}
            }
            out.push_str(&String::from_utf8_lossy(line.content()));
            true
        })?;
        Ok(out)
    }

    /// Create `branch` from `base` and check it out.
    pub fn create_branch(&self, branch: &str, base: &str) -> Result<()> {
        if self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .is_ok()
        {
            // A branch cannot be deleted while it is checked out; move away first.
            if self.current_branch().ok().as_deref() == Some(branch) {
                self.checkout(base)?;
            }
            self.delete_branch(branch)?;
        }
        let base_commit = self.branch_commit(base)?;
        self.repo
            .branch(branch, &base_commit, false)
            .with_context(|| format!("creating branch {branch}"))?;
        // `Branch::name` returns the short form ("main"), but `set_head`
        // resolves a symbolic target and needs the full reference path.
        let name = format!("refs/heads/{branch}");
        self.repo
            .set_head(&name)
            .context("checking out new branch")?;
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .context("checking out new branch")?;
        Ok(())
    }

    /// Delete a local branch.
    pub fn delete_branch(&self, branch: &str) -> Result<()> {
        let mut branch_ref = self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .with_context(|| format!("finding branch {branch}"))?;
        branch_ref
            .delete()
            .with_context(|| format!("deleting branch {branch}"))?;
        Ok(())
    }

    /// Check out an existing branch.
    pub fn checkout(&self, branch: &str) -> Result<()> {
        self.repo
            .find_branch(branch, git2::BranchType::Local)
            .with_context(|| format!("finding branch {branch}"))?;
        let name = format!("refs/heads/{branch}");
        self.repo.set_head(&name).context("setting HEAD")?;
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .context("checking out branch")?;
        Ok(())
    }

    /// Commit everything in the working tree with `message`.
    pub fn commit_all(&self, message: &str) -> Result<String> {
        let mut index = self.repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = self.repo.find_tree(tree_id)?;
        let signature = git2::Signature::new("fs-rs", "fs-rs@localhost", &now_seconds())?;
        let parents: Vec<git2::Commit<'_>> = match self.repo.head() {
            Ok(head) => vec![head.peel_to_commit()?],
            Err(_) => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        let oid = self.repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parent_refs,
        )?;
        Ok(oid.to_string())
    }

    /// Merge `branch` into the currently checked out branch using a fast-forward.
    ///
    /// The current branch ref is moved to the tip of `branch`; no extra commit
    /// is created. Fails when the current branch is not an ancestor of
    /// `branch`, because a fast-forward would not be possible then.
    pub fn merge_fast_forward(&self, branch: &str) -> Result<()> {
        let target = self.branch_commit(branch)?;
        let head = self.repo.head().context("reading HEAD")?;
        let head_name = head
            .name()
            .context("HEAD is detached; cannot merge")?
            .to_string();
        let current = head.peel_to_commit()?;
        if target.id() == current.id() {
            return Err(anyhow!(
                "branch {branch} is already merged into {head_name}"
            ));
        }
        // `graph_descendant_of(commit, ancestor)` answers "is `commit` a
        // descendant of `ancestor`", so the fast-forward condition is that the
        // merge target descends from the current head.
        if !self
            .repo
            .graph_descendant_of(target.id(), current.id())
            .unwrap_or(false)
        {
            return Err(anyhow!(
                "branch {branch} is not a descendant of {head_name}; a fast-forward merge is not possible"
            ));
        }
        if self.has_local_changes()? {
            return Err(anyhow!(
                "working tree has uncommitted changes; commit or stash them before merging"
            ));
        }
        let object = target.as_object().clone();
        self.repo
            .checkout_tree(&object, Some(git2::build::CheckoutBuilder::new().force()))
            .context("checking out merge target tree")?;
        let mut reference = self
            .repo
            .find_reference(&head_name)
            .with_context(|| format!("finding reference {head_name}"))?;
        reference
            .set_target(target.id(), "merge fast-forward")
            .with_context(|| format!("moving {head_name} to {}", target.id()))?;
        Ok(())
    }

    /// Whether the index or the working tree differs from `HEAD`.
    ///
    /// Untracked files are ignored: they neither block a merge nor belong to
    /// the commits being merged.
    pub fn has_local_changes(&self) -> Result<bool> {
        let mut options = git2::StatusOptions::new();
        options
            .include_untracked(false)
            .recurse_untracked_dirs(false);
        let statuses = self.repo.statuses(Some(&mut options))?;
        const DIRTY: git2::Status = git2::Status::INDEX_NEW
            .union(git2::Status::INDEX_MODIFIED)
            .union(git2::Status::INDEX_DELETED)
            .union(git2::Status::INDEX_RENAMED)
            .union(git2::Status::INDEX_TYPECHANGE)
            .union(git2::Status::WT_NEW)
            .union(git2::Status::WT_MODIFIED)
            .union(git2::Status::WT_DELETED)
            .union(git2::Status::WT_RENAMED)
            .union(git2::Status::WT_TYPECHANGE);
        Ok(statuses
            .iter()
            .any(|entry| entry.status().intersects(DIRTY)))
    }

    /// Working tree status, as `path -> status` pairs.
    pub fn status(&self) -> Result<Vec<(String, String)>> {
        let mut options = git2::StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        let statuses = self.repo.statuses(Some(&mut options))?;
        Ok(statuses
            .iter()
            .map(|entry| {
                (
                    entry.path().unwrap_or_default().to_string(),
                    format!("{:?}", entry.status()),
                )
            })
            .collect())
    }
}

/// Current unix timestamp in seconds, as required by git signatures.
pub fn now_seconds() -> git2::Time {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    git2::Time::new(seconds, 0)
}

/// Split a commit message into its summary line and the remaining body.
pub fn split_message(message: &str) -> (String, String) {
    let trimmed = message.trim_start_matches(['\n', '\r']);
    match trimmed.split_once("\n\n") {
        Some((summary, body)) => (summary.trim().to_string(), body.trim().to_string()),
        None => {
            let first_line = trimmed
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            (first_line, String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_message_separates_summary_and_body() {
        let (summary, body) = split_message("Fix crash\n\nDetails here\nmore");
        assert_eq!(summary, "Fix crash");
        assert_eq!(body, "Details here\nmore");
    }

    #[test]
    fn split_message_without_body_returns_single_line() {
        let (summary, body) = split_message("Only one line");
        assert_eq!(summary, "Only one line");
        assert!(body.is_empty());
    }

    #[test]
    fn split_message_handles_leading_newlines() {
        let (summary, _) = split_message("\n\nTitle\n");
        assert_eq!(summary, "Title");
    }
}
