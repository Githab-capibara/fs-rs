//! Synchronisation of tracked forks with their upstream remotes.
//!
//! The dashboard keeps a list of repositories. For every repository it needs
//! to know two things: how many upstream commits are still missing from the
//! fork's main branch, and what those commits are. This module fetches the
//! upstream branch, walks the commits that are not reachable from the fork and
//! converts them into the domain type [`UpstreamCommit`].

use anyhow::Result;

use crate::git::{GitRepo, PendingCommit};
use crate::{Repo, UpstreamCommit};

/// Build the browser URL of a commit inside the upstream repository.
///
/// `upstream_web` is the human readable address of the upstream project, for
/// example `https://github.com/owner/project`. The result points at the
/// commit page of the given object id.
pub fn commit_url(upstream_web: &str, sha: &str) -> String {
    let base = upstream_web.trim_end_matches('/');
    if base.is_empty() {
        return String::new();
    }
    format!("{base}/commit/{sha}")
}

/// Convert a pending commit into the domain type used by the dashboard.
pub fn to_upstream_commit(
    repo_id: i64,
    pending: PendingCommit,
    upstream_web: &str,
) -> UpstreamCommit {
    let url = commit_url(upstream_web, &pending.sha);
    UpstreamCommit {
        repo_id,
        sha: pending.sha,
        short_sha: pending.short_sha,
        summary: pending.summary,
        body: pending.body,
        author: pending.author,
        author_email: pending.author_email,
        authored_at: pending.authored_at,
        committed_at: pending.committed_at,
        files: pending.files,
        insertions: pending.insertions,
        deletions: pending.deletions,
        url,
    }
}

/// Fetch the upstream branch of `repo` and list the commits that the fork is
/// still missing.
///
/// The `upstream` remote is created or corrected first, so the caller only has
/// to provide the repository row it keeps in the store. The returned commits
/// are ordered newest first.
pub fn sync_repo(repo: &Repo) -> Result<Vec<UpstreamCommit>> {
    let git = GitRepo::open(&repo.path)?;
    if !repo.upstream_url.trim().is_empty() {
        git.ensure_upstream_remote(&repo.upstream_url)?;
    }
    git.fetch_upstream(&repo.main_branch)?;
    let pending = git.pending_commits(&repo.main_branch)?;
    Ok(pending
        .into_iter()
        .map(|commit| to_upstream_commit(repo.id, commit, &repo.upstream_web))
        .collect())
}
