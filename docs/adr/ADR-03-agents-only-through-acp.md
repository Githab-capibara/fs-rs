# 03. Drive the agents only through the Agent Client Protocol

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** @Githab-capibara
- **Related:** [ADR-02](ADR-02-opencode-mini-for-the-manual-terminal.md), [ADR-04](ADR-04-mock-agent-as-a-test-double.md)

## Context

The implementation pipeline has to run three agents that talk to each other:
the implementer ports the commit, the reviewer runs the linters and fixes what
they report, and the verifier approves or rejects the result. When the
verifier rejects, the feedback has to go back to the *same* reviewer session,
so that the reviewer keeps the context of its previous work.

The critical requirement is the continuity of the reviewer session. A pipeline
that starts a fresh agent for every step loses that context, and a pipeline
that drives the agent through its own private API breaks as soon as the agent
changes.

The Agent Client Protocol (ACP) is the protocol that OpenCode itself speaks:
newline-delimited JSON-RPC over stdio, with `initialize`, `session/new`,
`session/set_config_option` and `session/prompt`. It carries sessions, model
selection, streamed updates and permission requests. One session can receive
many prompts, which is exactly what the rework loop needs.

## Decision

We drive every automated agent through ACP and nothing else. `fs-rs` starts the
agent process, initialises the protocol, opens one session per agent role,
selects the model with `session/set_config_option` and sends the prompts with
`session/prompt`.

The dashboard answers the requests the agent sends back:

* `session/request_permission` is approved automatically, because the user
  explicitly asked for an autonomous pipeline,
* `fs/read_text_file` and `fs/write_text_file` are served inside the
  directories the user opened,
* `terminal/*` is served by a real pseudo terminal.

The only place where an agent is started outside of ACP is the manual terminal
of [ADR-02](ADR-02-opencode-mini-for-the-manual-terminal.md).

## Consequences

- **Easier:** The sessions keep their context across the rework loop, so the
  reviewer can be told "fix what the verifier found" without repeating the
  whole task. The protocol is the agent's own interface, so the pipeline
  survives changes in the agent. The model is chosen by an explicit call, not
  by a configuration file.
- **Harder:** ACP is more work than a shell pipeline: the client has to answer
  permission requests and file requests, and it has to keep the JSON-RPC
  streams in sync. The protocol is young, so its details may change.
- **Given up:** The freedom to call a private API of the agent. Nothing in
  `fs-rs` depends on the internals of OpenCode.
- **Migration:** The pipeline was written against ACP from the start; there is
  nothing to migrate.

## Alternatives considered

- **Option A:** run each agent as a shell command and parse its output.
  Rejected because the reviewer would lose its context on every rework cycle.
- **Option B:** use the OpenCode SDK directly. Rejected because it couples the
  dashboard to one agent and duplicates what ACP already does.
- **Option C:** talk to the agent over its HTTP API. Rejected because the HTTP
  API has no notion of a session that survives across prompts.
