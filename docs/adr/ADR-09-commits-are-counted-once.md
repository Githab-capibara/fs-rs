# 09. Count every commit once

- **Status:** Accepted
- **Date:** 2026-11-14
- **Deciders:** @Githab-capibara
- **Related:** [ADR-06](ADR-06-context-fitted-prompts.md),
  [`crates/fsrs-core/benches/sync.rs`](../../crates/fsrs-core/benches/sync.rs)

## Context

The dashboard is a poller: it fetches the upstream branch of every tracked fork
and lists the commits the fork is missing. The user sits in front of it while
it happens.

Two operations dominated that wait:

1. Listing the pending commits paid a tree-to-tree diff for every commit that
   `deleted..upstream` found, plus a graph walk for every commit found.
2. Storing the list deleted every row of the repository and inserted them
   again, so a second synchronisation of a fork that did not move rewrote the
   same 500 rows for nothing.

Both costs are paid again and again for the same immutable objects.

## Decision

A commit is immutable: its diff statistics never change after the first
listing. Therefore

* the statistics of a commit are computed once and cached by SHA, reused on
  every later synchronisation (`pending_commits_incremental`),
* the pending list is upserted by SHA instead of delete-and-insert, and a call
  whose list is already stored writes nothing at all (`replace_commits`).

## Consequences

- **Easier:** a warm synchronisation of a fork 200 commits behind takes 4.96 ms
  instead of 41.28 ms — a list that is already known is listed again without
  computing a single diff. Storing 500 rows that did not change costs 0.173 ms.
- **Harder:** the caller owns the statistics: the git layer cannot open the
  store, so the dashboard reads the cache on its own thread and hands it to the
  background thread — a SQLite connection is bound to one thread.
- **Given up:** the first synchronisation of a fork is not cheaper; it still
  walks every commit and computes every diff. Only the repetitions are.
- **Migration:** `sync_repo` takes the known statistics of the previous
  synchronisation, and the store is passed column by column, never shared
  across a thread boundary.

## Alternatives considered

- **Option A: keep the diff per commit.** rejected because it is the whole
  cost, repeated on every poll.
- **Option B: cache in memory only.** rejected because the dashboard restarts
  between polls; the store already persists and is the right place.
- **Option C: share the store with the background thread.** rejected because a
  SQLite connection is `!Send`; passing the statistics instead is explicit and
  safe.