# `fsrs-acp`

The Agent Client Protocol client and the three-agent pipeline. Every
automated agent is driven through ACP and nothing else, so a rework cycle
continues in the same session and the pipeline survives a change of the agent.

| Module | Content |
|--------|---------|
| [`agent`](src/agent.rs) | The JSON-RPC client: `initialize`, `session/new`, `session/set_config_option`, `session/prompt`, the permission requests and the file requests |
| [`pipeline`](src/pipeline.rs) | The cycle: the implementer ports the commit, the reviewer runs the linters and fixes, the verifier approves or rejects; the branch and the fast-forward merge |
| [`prompts`](src/prompts.rs) | The three prompts, built by pure functions so that they can be asserted and shown before they are sent |
| [`models`](src/models.rs) | The list of the models that the agent offers, and the filter that keeps the retired `local/oxcoder-9b` out of it |
| [`events`](src/events.rs) | `AgentRole` and `RunEvent`: every observable step of a run |
| [`terminal`](src/terminal.rs) | The pseudo terminals that answer the `terminal/*` requests of the protocol |
| [`mock`](src/mock.rs) | The mock agent: the test double that plays the three roles |

| Binary | What it is |
|--------|------------|
| [`src/bin/mock_agent.rs`](src/bin/mock_agent.rs) | The mock agent as a process — the dashboard starts it as a separate agent |

| Test suite | What it covers |
|------------|----------------|
| [`tests/pipeline_mock.rs`](tests/pipeline_mock.rs) | The whole pipeline against the mock: the prompts, the rework loop, the branch and the merge |
| [`tests/live_opencode.rs`](tests/live_opencode.rs) | The client against a running `opencode acp`. The tests skip themselves when OpenCode is not installed |

## The flow

```text
implementer ──prompt──► reviewer ──prompt──► verifier
     ▲                     │  ▲                  │
     └───── feedback ──────┘  └──── reject ─────┘
```

Three sessions are opened once. The reviewer keeps its context across the
rework cycles, so the feedback of the verifier reaches the same session that
produced the rejected result.

## Related documents

| Document | Purpose |
|----------|---------|
| [ADR-03](../../docs/adr/ADR-03-agents-only-through-acp.md) | Why ACP and nothing else |
| [ADR-04](../../docs/adr/ADR-04-mock-agent-as-a-test-double.md) | Why the mock agent lives in the library |
| [Design note — the pipeline](../../docs/design/02-three-agent-pipeline.md) | The cycle, the prompts, the branch and the merge |
