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

## How to read an ADR

* **Status** — `Proposed`, `Accepted`, `Deprecated` or `Superseded by ADR-NN`.
* **Deciders** — the GitHub handles that took the decision.
* **Consequences** — what the decision makes easier, harder, and what it gives
  up.
* **Alternatives considered** — the options that were rejected and why.

An ADR is never rewritten after it is accepted. A change of mind produces a new
ADR that supersedes the old one.
