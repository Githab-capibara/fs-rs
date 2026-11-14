# 07. Serialize the live tests of the external services

- **Status:** Accepted
- **Date:** 2026-11-11
- **Deciders:** @Githab-capibara
- **Related:** [ADR-04](ADR-04-mock-agent-as-a-test-double.md), [ADR-05](ADR-05-foq-as-the-only-local-model.md), [ADR-06](ADR-06-context-fitted-prompts.md)

## Context

The suite has two kinds of tests. The tests against a test double are
deterministic and independent of each other. The live tests talk to a real
service: `crates/fsrs-ai/tests/live_server.rs` talks to the decision server,
`crates/fsrs-acp/tests/live_opencode.rs` talks to `opencode acp`.

Both of them failed when the suite ran in parallel, although every one of them
passed on its own.

The decision server runs with a single slot (`-np 1`, see
[ADR-06](ADR-06-context-fitted-prompts.md)). The window of the runtime is
shared between the slots, and the slot is the unit that serves a request. Four
tests that ask the model at the same time queue up behind each other, and the
request that waits answers last. The client times out after
`foq_timeout_secs` — 120 seconds — and the test fails with
`Http(Timeout(Global))`. The failure says nothing about the code under test;
it says that four requests shared one slot.

The agent tests have the same shape for a different reason. Every one of them
starts its own `opencode acp` process, and all of them use the account of the
user. The provider refuses the requests that arrive together, so the prompt of
one test is rejected while the others are in flight.

Both failures are the same failure: the tests compete for a resource that can
serve one client at a time.

## Decision

We serialize the live tests of each service with a lock that they take before
they talk to it:

* `crates/fsrs-ai/tests/live_server.rs` holds a `static SLOT:
  LazyLock<Mutex<()>>` — the single slot of the decision server,
* `crates/fsrs-acp/tests/live_opencode.rs` holds a `static AGENT:
  LazyLock<Mutex<()>>` — the agent, one process and one account at a time.

The lock is taken inside the test and held until the test ends, so the
requests queue up instead of competing. The tests that only read the state of
a service — `/props`, `/tokenize` — do not take it, because they do not
occupy the slot.

The timeout of the client is not a substitute for the lock. The timeout
protects the user of the dashboard from a runtime that never answers; the lock
protects the tests from each other. Both are needed, and they answer different
questions.

## Consequences

- **Easier:** The live tests are deterministic again: they pass or they find a
  real defect, and the failure of one of them is not a symptom of the load of
  the others. The suite can be run with the default number of threads.
- **Harder:** The live tests take longer — the requests wait for each other
  instead of running at the same time. The two suites are the slow part of
  `cargo test --workspace`, and their duration depends on the speed of the
  local accelerator and of the provider.
- **Given up:** The parallelism of the live tests. A runtime with several slots
  could serve the tests of the decision model at the same time, but the
  guarantee of the suite would then depend on how the runtime was started.
- **Migration:** None. The lock is local to the test files.

## Alternatives considered

- **Option A:** raise the timeout of the client until the requests of the
  parallel tests fit. Rejected because the timeout would have to cover the
  whole queue of the slot, and a test that waits for the others would be as
  slow as the sum of their times.
- **Option B:** give the runtime more slots so that the tests fit in parallel.
  Rejected because the window of the runtime is shared between the slots, so
  every slot would get a smaller window — the problem that
  [ADR-06](ADR-06-context-fitted-prompts.md) was written about.
- **Option C:** run the live tests in a separate job of the CI, one test at a
  time. Rejected because the live tests are skipped on the runner anyway: the
  runner has neither the decision model nor the agent.
- **Option D:** drop the live tests and keep only the tests against the test
  double. Rejected because the live tests are the only ones that prove that
  the client speaks the protocol of the real runtime and of the real agent.
