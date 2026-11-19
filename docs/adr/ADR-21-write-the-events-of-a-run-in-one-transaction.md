# 21. Write the events of a run in one transaction

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-19](ADR-19-draw-only-the-visible-rows.md),
  [`crates/fsrs-core/src/store.rs`](../../crates/fsrs-core/src/store.rs),
  [`crates/fsrs-app/src/worker.rs`](../../crates/fsrs-app/src/worker.rs)

## Context

A pipeline of three agents produces a burst of events: the prompt that was
sent, the message of the agent, its reasoning, the calls of its tools. The
worker wrote every one of them into the database on its own, and it wrote the
state of the run in a second transaction.

Two things were wrong with it. The first is the cost: a transaction of
SQLite in the mode of the dashboard is a commit, and a commit waits for the
disk. A pipeline that produces two hundred events asked the disk two hundred
times, and the thread that waits for the disk is the thread of the background
that has to fetch the answer of the next agent.

The second is the order. The event and the state that it produced were two
separate transactions, and the program could stop between them: the database
could hold the event that says that the reviewer started, and a run that still
says that the implementer is working. The reader of the database sees a run
that never happened.

## Decision

The events that arrive together are written in one transaction, and the state
of the run is written in the same transaction as the events that produced it.

**The worker batches the events.** The events arrive through a channel, and
the worker takes everything that is already there — up to sixty-four events —
and writes it in one call of the store. The burst of a pipeline becomes a
handful of transactions instead of one transaction per event.

**The state belongs to the events.** The state of the run is the newest one
that the events produced, and it is written with them, in the same
transaction. The state and the events that explain it can never be out of
step.

**The store offers both ways.** `insert_run_events` writes a batch of events,
and `insert_run_events_with_state` writes the batch and the state of the run
together. The two are the same transaction; the second one only writes the
state as well.

## Consequences

- **Easier:** the disk is asked to commit once for a burst of events, and the
  state of a run and the events of that run are always in step. The test
  `the_events_and_the_state_of_a_run_are_written_together` of
  [`crates/fsrs-core/src/store.rs`](../../crates/fsrs-core/src/store.rs) proves
  it, and the test
  `the_events_of_the_run_are_written_in_one_batch` of
  [`crates/fsrs-app/src/worker.rs`](../../crates/fsrs-app/src/worker.rs) proves
  that the worker writes the events that it collects.
- **Harder:** the worker holds the events of a batch in memory until it writes
  them, and the state of the run is the newest one of the batch. A batch of
  sixty-four events of a pipeline is a few kilobytes.
- **Given up:** the events of a pipeline reach the database in bursts, and not
  one by one. A reader of the database that looks at the events of a running
  pipeline sees the events of the last burst.
- **Migration:** none. The rows of the database hold the same data, and the
  program reads them the same way.

## Alternatives considered

- **Option A:** write every event in its own transaction. Rejected: the disk
  is asked to commit once per event, and the state of the run can disagree
  with the events that explain it.
- **Option B:** write the events in one transaction, and the state in another.
  Rejected: the state would still be able to disagree with the events, and
  the state is written once per burst anyway.
- **Option C:** keep the events of a run in memory and write them when the
  pipeline is over. Rejected: the log of the pipeline is what the reader
  watches while it runs, and a program that keeps the log in memory loses it
  when it stops.
