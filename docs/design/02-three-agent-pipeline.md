# 02. The three-agent pipeline

- **Status:** Design note
- **Date:** 2026-11-09
- **Deciders:** @Githab-capibara
- **Researcher:** fs-rs
- **Purpose:** Explain how one upstream commit is ported into a fork, reviewed and verified.
- **Feeds into:** [ADR-03](../adr/ADR-03-agents-only-through-acp.md), [ADR-04](../adr/ADR-04-mock-agent-as-a-test-double.md)

## The cycle

```text
          ┌──────────────────────────────────────────────┐
          │                                              │
          ▼                                              │
  ┌───────────────┐   prompt    ┌───────────────┐       │
  │  implementer  │ ──────────► │  reviewer     │       │
  │  (port the    │ ◄────────── │ (linters +    │       │
  │   change)     │  feedback   │  fixes)       │       │
  └───────────────┘             └───────┬───────┘       │
          │                             │ prompt        │
          │                             ▼               │
          │                      ┌───────────────┐       │
          │                      │  verifier     │       │
          │                      │ (approve or   │       │
          │                      │  reject)      │───────┘
          │                      └───────────────┘
          │                            reject
          │                             feedback
          └─────────────────────────────┘
```

Three sessions are opened once, at the start of the run: one for the
implementer, one for the reviewer, one for the verifier. The sessions are
never closed while the run lasts, so the reviewer keeps the context of its
previous work when the verifier rejects the result.

## The prompts

Every prompt is built by a pure function in `fsrs-acp::prompts`, so it can be
asserted in a unit test and shown to the user before it is sent.

**The implementer** receives the link of the upstream commit, its title, its
author, its description, the list of the touched files and the diff, truncated
to 20 000 characters while keeping the beginning and the end.

**The reviewer** receives the commit and the list of the linters to run:
`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `shellcheck`,
`oxlint`/`biome` and `typos`, with `cargo audit` and `cargo deny` as
optional extras. The prompt asks the reviewer to fix everything they report —
the dashboard does not run the linters itself, it asks the agent to do it.

**The verifier** receives the commit and the instruction to judge the result.
It has to print a verdict on a line of its own:

```text
VERDICT: APPROVED
```

or

```text
VERDICT: REJECTED
the parser still mishandles an empty input
```

The last marker wins, so an agent that mentions both outcomes in its reasoning
still produces an unambiguous result. The lines that follow the final verdict
are the reasons, and they are what the reviewer receives on the next cycle.

When the verifier rejects without giving a reason, the pipeline invents one:
*the verifier rejected the implementation at cycle N without giving a reason*.
The reviewer needs something to work on.

## The branch

The implementation is created on `fsrs/implement/<short-sha>`. The branch is
kept after the merge, so the work of a run stays visible in the history of the
repository.

Before the merge, the pipeline checks whether the working copy carries
uncommitted changes. If it does — the agent wrote something and did not commit
it — those changes are committed on the branch. Only then is the branch merged
into `main`, and only with a fast-forward merge: the commit tree is written
directly onto the branch and the reference is moved. A merge that rewrites
history is never created.

## The events

Every observable step is a `RunEvent`: the start of the run, every prompt that
was sent, every answer of an agent, the rejection with its feedback, the
approval and the merge. The events are sent through an unbounded channel to
the dashboard, which renders them in the log panel, and they are persisted, so
a run can be replayed after the window was closed.

## The limits

* `DEFAULT_MAX_CYCLES = 5`. A sixth rejection ends the run with
  `PipelineError::TooManyCycles` and the branch is left as it is.
* A verifier that never prints a verdict ends the run with
  `PipelineError::MissingVerdict`.
* The permission requests of the agents are approved automatically, because the
  user asked for an autonomous pipeline.
