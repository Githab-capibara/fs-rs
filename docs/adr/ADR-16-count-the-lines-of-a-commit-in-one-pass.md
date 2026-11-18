# 16. Count the lines of a commit in one pass

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-12](ADR-12-read-the-objects-as-fast-as-git-can.md),
  [`crates/fsrs-core/src/git.rs`](../../crates/fsrs-core/src/git.rs),
  [`crates/fsrs-core/examples/revwalk-probe.rs`](../../crates/fsrs-core/examples/revwalk-probe.rs)

## Context

The dashboard lists the commits that a fork is missing and shows how many
lines every one of them adds and removes. Those numbers come from the diff of
the commit against its first parent, and the diff is the expensive part of the
listing: it is the comparison of two trees and the reading of every changed
line.

The library that reads the repository, `libgit2`, can count the lines itself —
`git_diff_stats` walks the diff and returns the number of the insertions and
of the deletions. The dashboard needs the names of the changed files as well,
and it took them from a second walk of the same diff: one walk to collect the
names of the files, and another one to let the library count the lines.

Two walks of one diff cost twice what one walk costs. The listing of a fork
with a long backlog is paid for every time the upstream is fetched, and it is
the only real wait of the program.

The measurement is on the fork of the `fd` project, which is 1450 commits
behind its upstream. The probe
[`revwalk-probe`](../../crates/fsrs-core/examples/revwalk-probe.rs) lists the
missing commits and separates the two halves of the work: the metadata of the
commits, which reads one object per commit, and the statistics, which compares
two trees per commit.

| The listing of 1450 commits | Before | After |
|-----------------------------|--------|-------|
| The metadata | 25 ms | 24 ms |
| The statistics | 1878 ms | 995 ms |
| The whole listing | 1903 ms | 1018 ms |

The statistics of the listing are half of what they were. The numbers are the
medians of three runs of the probe on the machine that wrote this record.

The benchmark of the library agrees: the statistics of one commit of the
fixture of two hundred commits cost 146.40 µs before the change and 107.74 µs
after it, and the listing of the two hundred commits costs 32.53 ms and
23.60 ms. Both numbers were measured on the same machine with the same
compiler — see
[the benchmarks](../benchmarks/README.md#benchmark-of-the-synchronisation-of-a-fork).

## Decision

The program counts the lines of a diff itself, in the one walk it makes
anyway.

`libgit2` hands every line of a diff to the callback of the program, and the
callback receives the origin of the line: `+` for a line that was added, `-`
for a line that was removed, and a space for a line that stayed. The program
counts the origins it cares about and ignores the rest. The names of the
changed files come from the same walk, from the callback of the files.

The count of the program is the count of `libgit2`: the test
`the_statistics_of_a_commit_are_the_ones_of_git_itself` compares the numbers
of the program with the numbers that `git show --numstat` prints, for a commit
that adds lines, for a commit that removes a file and for a commit that
changes a file in the middle.

A binary file has no lines to count, so it adds nothing to the statistics —
the same as the count of `libgit2` and the same as the count of `git`.

## Consequences

- **Easier:** the statistics of a commit come from one walk of the diff, and
  the test that compares them with the numbers of `git` keeps them honest.
  The probe that measures the listing tells how much of the time is the
  metadata and how much is the diff, so the next change of the listing is
  measured against a known number.
- **Harder:** the program now knows how a diff is counted, and it has to keep
  knowing it: the origin of a line is part of the contract of the callback,
  and a change of the library would have to be checked against `git` again.
- **Given up:** the second walk of the diff is given up, and with it the
  guarantee that the count of the library is the count of the program — the
  program counts the lines now, and the test is what keeps the two the same.
- **Migration:** none. The rows that are already in the database hold the
  numbers of the old count, and the two counts are the same.

## Alternatives considered

- **Option A:** keep the count of `libgit2` and drop the second walk. Rejected:
  the library counts the lines in a walk of its own, and the names of the
  files come from another one — the two walks are the cost, not the
  duplication of the counting.
- **Option B:** count the lines outside of `libgit2`, by reading the blobs of
  the two versions of every file and comparing them. Rejected: it reads every
  changed file twice and writes the comparison itself, and the library does
  the same work in C with the same result.
- **Option C:** compute the statistics of the commits with the command line of
  `git` instead of with the library. Rejected: the program would have to
  start a process for every commit, and the library is already a dependency
  of the project.
