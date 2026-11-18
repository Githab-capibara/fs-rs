# 12. Read the objects of a fork as fast as git can read them

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-11](ADR-11-build-for-the-machine.md),
  [`crates/fsrs-core/examples/revwalk-probe.rs`](../../crates/fsrs-core/examples/revwalk-probe.rs)

## Context

The dashboard reads the objects of a tracked fork over and over. A
synchronisation walks the commits that the fork does not have yet and
compares the trees of every one of them; the rating of a commit builds the
patch of the commit; the pipeline reads the diff again.

Two options of git2 stand in the way of that pattern:

* `enable_caching` keeps the parsed objects of the repository in a cache. The
  objects of a commit are read once and thrown away here, so the cache costs
  memory and time instead of saving them.
* `strict_hash_verification` checks the hash of every object that is read.
  The check belongs to a verification of the repository, not to a working copy
  that was fetched over https: the transport and the pack index have already
  verified it.

The eleventh point of the checklist of the performance work asks for these two
options to be turned off, but only after a measurement, and only with a clear
understanding of the price.

The measurement was made on a real fork: the `fd` project, 1450 commits
behind its upstream, every commit touching real files.

| options | listing of the commits | of which the diff |
|----------|-----------------------|-------------------|
| both on (default) | 2500 ms | 2467 ms |
| verification off | 2320 ms | 2261 ms |
| caching off | 2450 ms | 2368 ms |
| both off | **1270 ms** | 1226 ms |

The numbers are the medians of five runs of
`cargo run -p fsrs-core --example revwalk-probe -- <fork> <branch>`.

The measurement also corrects the assumption of the checklist: the walk of
the graph is **not** the expensive part. It takes 30 ms of the 2500 ms; the
comparison of the trees of every commit takes the other 2467 ms. The options
halve the time of the listing because they make the diff cheaper, not because
they make the walk cheaper.

The two options are only fast together. Turning one of them off alone leaves
the time of the listing unchanged.

## Decision

The two options of git2 are turned off once, when the program starts:

```rust
fsrs_core::git::configure_performance();
```

The call is in [`crates/fsrs-core/src/git.rs`](../../crates/fsrs-core/src/git.rs)
and is called from `main.rs`, before any repository is opened.

## Consequences

- **Easier:** the listing of the missing commits of a fork that is behind by
  1450 commits takes 1.27 s instead of 2.50 s — it is twice as fast. The
  synchronisation of a big backlog is the operation the user waits for, and
  the gain is the whole time of the diff.
- **Harder:** the options are global to the process. A program that wants the
  verification of the objects cannot have it after the call — the check has
  to be turned back on with `git2::opts::strict_hash_verification(true)`.
- **Given up:** an object whose content does not match its name is not
  detected while it is read. A repository that was fetched over https is
  verified by the transport, and `git fsck` still checks the whole repository
  when the user asks for it. This is the level of the integrity of the
  ordinary `git` command.
- **Migration:** none. The change is inside the program; the repositories on
  disk are not touched.

## Alternatives considered

- **Option A:** turn the options off only when the listing is big. Rejected
  because the options are global to the process and a program that changes
  them in the middle of its life has two sets of behaviour.
- **Option B:** keep the verification and make the diff cheaper another way —
  compute the statistics of a commit only once and keep them in the store.
  Accepted as well, and done: the cache of the statistics by SHA is the
  change of the eighth point of the checklist. It removes the diff from every
  listing but the first one; the options of this record remove it from the
  first one too.
- **Option C:** compute the statistics lazily, when the interface asks for
  them. Rejected for now: the interface shows the size of every commit right
  away, so the statistics are needed at once. The metadata of a commit can be
  read without any diff, and the split of the two halves is in the code — the
  interface can ask for the metadata first if it ever needs to.
