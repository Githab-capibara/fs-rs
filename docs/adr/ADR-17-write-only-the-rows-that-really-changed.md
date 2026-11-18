# 17. Write only the rows that really changed

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-09](ADR-09-commits-are-counted-once.md),
  [`crates/fsrs-core/src/store.rs`](../../crates/fsrs-core/src/store.rs),
  [`crates/fsrs-core/benches/sync.rs`](../../crates/fsrs-core/benches/sync.rs)

## Context

The dashboard polls the upstream of every repository it follows. After every
poll it writes the list of the pending commits back into the database, so the
state of the program survives a restart.

The write used to be the whole list. Every commit of the list was written
again — its message, its author, its dates, the names of its files and the
statistics of its diff — and the commits that were no longer pending were
deleted by a statement that compared every row with the list of the object
ids. A repository with a backlog of five hundred commits wrote five hundred
rows on every poll, and most of them were exactly the rows that were already
there.

A commit is immutable. The object id of a commit names its tree, its parents
and its metadata, and the diff of a commit against its first parent is the
same forever. The statistics of a commit that was listed yesterday are the
statistics of the same commit today. Rewriting them is a page of the database
for nothing.

The measurement is of the store of the benchmark, with five hundred rows of
commits, on a database on the disk — the dashboard opens its database on the
disk, and the write of a page is a real cost there. The "before" column is
the code of the fifth wave, measured again on the same machine with the same
compiler.

| The store of 500 rows | Before | After |
|-----------------------|--------|-------|
| The same list | 157.76 µs, one write of the count | 164.82 µs, no write at all |
| One commit more | every row was written again, 378.65 µs | 222.20 µs, one row is written |
| One commit less | every row was written again | 405.01 µs, one row is written away |
| The address of the upstream changed | every row was written again | 356.96 µs, every address is rewritten |

The times are the medians of ten runs of Criterion. The first line is the one
that matters: it is what every poll of a repository that did not move costs,
and it is the same as before — the store does the same work to find out that
nothing changed, and it writes nothing at all afterwards.

## Decision

The store writes the difference between the list it has and the list it
receives.

One statement reads the object ids of the commits of the repository. The
commits that are not in the store are inserted, the commits that are no longer
pending are deleted by their object id, and the commits that are already
stored are left alone. The number of the pending commits is written only when
it is not the number that is already stored.

The address of the web page of a commit is the one thing of a stored commit
that can change: it carries the address of the upstream, and the user can
change the address of the upstream in the configuration. The address follows
the address of the repository, and the store rewrites it in one statement when
the repository changes — the address of a commit is its beginning and its
object id, so the statement is a concatenation.

The counter of the writes of SQLite is part of the interface of the store, so
that the promise can be checked: the test `the_same_list_is_written_once`
counts the rows that were written and asks for the same list again, and the
counter does not move.

## Consequences

- **Easier:** a poll of a repository that did not move writes nothing, and
  the store says so with a number instead of a promise. The rows that are
  already correct are not rewritten, so the database keeps the rows it has.
- **Harder:** the store assumes that a commit does not change. The assumption
  holds for git — the object id of a commit names its content — and the
  assumption is written down where the store is written. A store that had to
  carry mutable data would have to write every row again.
- **Given up:** the store does not repair a row that was written wrong by an
  older version of the program, and the rows of the commits that are not
  pending are deleted instead of being kept for a later listing.
- **Migration:** none. The rows that are in the database hold the same values
  they held before, and the address of the commits is rewritten the first time
  the address of the upstream changes.

## Alternatives considered

- **Option A:** keep the upsert of every row. Rejected: it writes five hundred
  rows to store the values that are already stored, and the values of a commit
  never change.
- **Option B:** delete every row of the repository and insert the list again.
  Rejected: it is the path the store used at the beginning, and it is four
  times slower than writing one row — measured, the numbers are in the table
  above and in the benchmark.
- **Option C:** compare the rows of the store with the list in SQL, with the
  `json_each` function of SQLite. Rejected: the membership of the list was
  tested for every row of the repository, and the comparison belongs to the
  program, which already has the list in memory.
