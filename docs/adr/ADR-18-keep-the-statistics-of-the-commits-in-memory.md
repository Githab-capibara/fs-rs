# 18. Keep the statistics of the commits in memory

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-09](ADR-09-commits-are-counted-once.md),
  [ADR-12](ADR-12-read-the-objects-as-fast-as-git-can.md),
  [`crates/fsrs-app/src/worker.rs`](../../crates/fsrs-app/src/worker.rs)

## Context

The statistics of the diff of a commit never change: the object id of a
commit names its tree and its parents. The dashboard therefore keeps them
between two synchronisations and reuses them, so that a fork that did not move
is listed without a single diff. That is what ADR-09 decided.

The statistics were kept in the database, and the interface read them back
before every synchronisation. The read was one query for every repository,
and it parsed the list of the files of every commit again — the files are
stored as a JSON text, and the program has to read them to know which files a
commit touched. The parse of the list of the files of every pending commit
was paid on every poll, for a list that the program already had in memory.

The same work was paid twice in the git layer: the statistics of a commit
that was already known were copied out of the map, one commit after another,
and the lists of the files of a commit are the bulk of a statistic.

The read of the database costs 125.46 µs for the two hundred commits of the
benchmark, and it was paid on every poll of every repository. The copy of the
lists of the files was paid on every commit of every poll.

## Decision

The background worker keeps the statistics of the commits of every repository.

The worker is the one that owns the synchronisation: it takes the statistics
out of its cache, hands them to the git layer, and fills the cache again from
the result of the listing. The git layer takes the statistics out of the map
it is given — it moves them into the result of the listing instead of copying
them — so the lists of the files are not copied a second time.

The interface hands the statistics over only once: when it opens the
database, it reads the commits of every repository anyway, and the statistics
of those commits are the statistics of the commits themselves. The database is
not asked for them a second time.

## Consequences

- **Easier:** the poll of a repository does not read the database any more,
  and the statistics are moved from one place to another instead of being
  copied. The worker knows the statistics of every repository, and the
  interface does not have to know where they are kept.
- **Harder:** the worker holds the statistics of every repository in memory.
  The lists of the files of a thousand commits are a few hundred kilobytes,
  and the cache is dropped when the worker is dropped.
- **Given up:** the statistics are not read back from the database when the
  worker is built. A program that opened the database and never synchronised
  would not have the statistics — the dashboard synchronises every repository
  when it opens, so it has them a moment later.
- **Migration:** none. The rows of the database hold the same statistics, and
  the first synchronisation reads them out of the commits of the database.

## Alternatives considered

- **Option A:** keep the statistics in the database and read them before every
  synchronisation. Rejected: the database is the place where the state
  survives a restart, not the place where the state of a running program is
  kept, and the read parsed the list of the files of every commit again.
- **Option B:** keep the statistics in the interface, next to the commits.
  Rejected: the interface would have to hand a copy of them to the worker,
  because the worker owns the thread that does the work — and the copy of
  the lists of the files is exactly what the decision avoids.
- **Option C:** compute the statistics of every commit again on every
  synchronisation. Rejected: that is the work the cache exists to avoid, and
  it is the most expensive part of the listing.
