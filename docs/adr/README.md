# Architecture Decision Records

Decisions that shape `fs-rs` and the reasons behind them. The format is
Michael Nygard's: context, decision, consequences, alternatives.

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-01](ADR-01-horizontal-repository-strip.md) | Show the repositories in a horizontal strip | Accepted |
| [ADR-02](ADR-02-opencode-mini-for-the-manual-terminal.md) | Run `opencode mini` in the manual terminal | Accepted |
| [ADR-03](ADR-03-agents-only-through-acp.md) | Drive the agents only through the Agent Client Protocol | Accepted |
| [ADR-04](ADR-04-mock-agent-as-a-test-double.md) | Keep the mock agent in the library | Accepted |
| [ADR-05](ADR-05-foq-as-the-only-local-model.md) | Use Foq as the only local decision model | Accepted |
| [ADR-06](ADR-06-context-fitted-prompts.md) | Fit every prompt into the window of the runtime | Accepted |
| [ADR-07](ADR-07-serialize-the-live-tests.md) | Serialize the live tests of the external services | Accepted |
| [ADR-09](ADR-09-commits-are-counted-once.md) | Count every commit once | Accepted |
| [ADR-10](ADR-10-the-decision-server-is-asked-four-times.md) | Ask the decision server four times, not thirty | Accepted |
| [ADR-11](ADR-11-build-for-the-machine.md) | Build the dashboard for the machine it runs on | Accepted |
| [ADR-12](ADR-12-read-the-objects-as-fast-as-git-can.md) | Read the objects of a fork as fast as git can read them | Accepted |
| [ADR-13](ADR-13-rate-the-commits-one-at-a-time.md) | Rate the commits one at a time | Accepted |
| [ADR-14](ADR-14-keep-egui-036-while-the-terminal-widget-needs-it.md) | Keep egui 0.36 while the terminal widget needs it | Accepted |
| [ADR-15](ADR-15-lay-the-code-out-again-with-bolt.md) | Lay the code out again with BOLT | Accepted |

## How to read an ADR

* **Status** — `Proposed`, `Accepted`, `Deprecated` or `Superseded by ADR-NN`.
* **Deciders** — the GitHub handles that took the decision.
* **Consequences** — what the decision makes easier, harder, and what it gives
  up.
* **Alternatives considered** — the options that were rejected and why.

An ADR is never rewritten after it is accepted. A change of mind produces a new
ADR that supersedes the old one.
