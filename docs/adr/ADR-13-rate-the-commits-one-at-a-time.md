# 13. Rate the commits one at a time

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-10](ADR-10-the-decision-server-is-asked-four-times.md),
  [`crates/fsrs-ai/examples/slot-probe.rs`](../../crates/fsrs-ai/examples/slot-probe.rs)

## Context

The dashboard has a backlog of commits to rate. The rating of one commit is
two questions to the local model — the criticality and the complexity — and
the model is a single process on a single GPU.

The seventeenth point of the checklist of the performance work asks for the
ratings to be evaluated in parallel, so that the time of the backlog is the
time of the slowest commit instead of the sum of all of them.

The server was measured before the decision was taken. The runtime that serves
the model reports one slot:

```json
{"total_slots": 1, "n_ctx": 4096}
```

One slot means one request at a time. The requests of a parallel run are
therefore not evaluated at the same time — they wait for each other. The
measurement confirms it:

| Requests | One after another | In parallel |
|----------|-------------------|-------------|
| 1 | 75 ms | 78 ms |
| 2 | 155 ms | 190 ms |
| 4 | 342 ms | 394 ms |
| 8 | 743 ms | 797 ms |

The parallel run is **slower** at every size: the requests spend their time in
the queue of the server and the threads only add the cost of the context
switch. The numbers are the medians of five runs of
`cargo run -p fsrs-ai --example slot-probe -- <url>` against the local
server.

The same server answered the benchmark of 60 commits of `fd` at a median of
8.9 s per rating — the time is the model, not the transport.

## Decision

The number of the ratings that are in flight never exceeds the number of the
slots of the server, and never exceeds the constant of the program:

```rust
fn concurrent_assessment_limit(slots: usize) -> usize {
    MAX_CONCURRENT_ASSESSMENTS.min(slots.max(1))
}
```

The number of the slots is asked from the server once, in the background, and
the answer is remembered by the client. A server that does not answer is
assumed to have one slot — the safe assumption, because a request that waits
in the queue of the server waits for the model anyway.

With the server of this machine the limit is one rating at a time.

The measurement is kept in the repository as
[`crates/fsrs-ai/examples/slot-probe.rs`](../../crates/fsrs-ai/examples/slot-probe.rs),
so the decision can be checked again when the server or the hardware changes.

## Consequences

- **Easier:** the backlog of a fork is rated in the time of the model, and
  the memory of the process holds one prompt at a time. The logs of a run are
  in the order of the commits, which makes a failed run easy to read.
- **Harder:** a server with several slots is not used to the full. The
  backlog of a fork with a thousand commits takes as long as the model needs
  for a thousand commits.
- **Given up:** the possibility that the slowest commit of a batch sets the
  time of the whole batch. The parallel path would have given that, if the
  server had had the slots.
- **Migration:** none. The change is inside the program: the repositories, the
  ratings and the reports are the same as before.

## Alternatives considered

- **Option A:** rate the commits in parallel, one thread per commit.
  Rejected because the measurement shows that the server has one slot: the
  threads only queue the requests and make the run slower.
- **Option B:** keep the parallel path, but limit it to the number of the
  slots that the server reports. Accepted as the way to write the code, and
  done: the number of the slots is asked once per process, and the number of
  the ratings in flight never exceeds it. With one slot the program behaves
  exactly like the sequential path, and with a bigger server it uses what is
  there.
- **Option C:** use several models on several GPUs. Rejected for now: the
  program is built around one local model, and the first run of the runtime
  on a machine with more than one accelerator is not known.
