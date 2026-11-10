# 04. Keep the mock agent in the library

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** @Githab-capibara
- **Related:** [ADR-03](ADR-03-agents-only-through-acp.md)

## Context

The pipeline and the dashboard both need an agent to talk to. A real agent
makes the tests slow, expensive and non-deterministic: it needs a network, a
model, and minutes per run.

The tests need to assert on the *behaviour* of the client, not on the answers
of a model: that the implementer receives the link, the description and the
diff; that the reviewer receives the findings of the verifier on the second
cycle; that the branch is merged into `main` only after an approval; that a
rejection loops back to the same reviewer session.

A test double that speaks the same protocol answers all of that.

## Decision

We keep the mock agent in the library, in `fsrs_acp::mock`, behind the
[`MockOptions`] type and the [`serve`] function. Two thin binaries wrap it:

* `crates/fsrs-acp/src/bin/mock_agent.rs`,
* `crates/fsrs-app/src/bin/fsrs-mock-agent.rs`.

The pipeline tests start the mock in-process; the dashboard tests start the
`fsrs-mock-agent` binary, because the dashboard spawns the agent as a separate
process.

The mock plays the three roles: the implementer writes and commits a file, the
reviewer reports that the linters pass, and the verifier rejects the first
attempt and approves the second one.

## Consequences

- **Easier:** Every crate can start the mock without a helper binary, so the
  same fixture is reused by the pipeline tests and by the dashboard tests. The
  mock is deterministic, so a failure in a test is always a failure in the
  client. The suite runs without a network and without a model.
- **Harder:** The mock is a second implementation of the protocol. It has to be
  kept in step with the client, or the tests can pass against a protocol that
  the real agent does not speak.
- **Given up:** The tests do not exercise a real agent. That gap is covered by
  `crates/fsrs-acp/tests/live_opencode.rs`, which runs against the installed
  `opencode acp` and is skipped when it is absent.
- **Migration:** None.

## Alternatives considered

- **Option A:** a mock that lives only in the test directory of one crate.
  Rejected because the dashboard tests could not reuse it.
- **Option B:** record the traffic of a real agent and replay it. Rejected
  because a recording cannot model the rework loop: the second prompt depends
  on the answer to the first one.
- **Option C:** test only against the real agent. Rejected because the tests
  would need a model and a network, and would be too slow to run on every
  change.
