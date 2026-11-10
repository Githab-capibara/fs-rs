# 02. Run `opencode mini` in the manual terminal

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** @Githab-capibara
- **Related:** [ADR-03](ADR-03-agents-only-through-acp.md), [configuration guide](../guides/02-configuration.md)

## Context

Clicking a commit opens a real terminal on the right of the window. The user
first picks a model from the models that OpenCode has registered, and the
terminal then has to run that model.

The full `opencode` user interface was the obvious candidate. It was measured
against the installed version (2.0.32) and turned out to be unsuitable:

* it accepts no `--model` flag,
* it ignores the `OPENCODE_CONFIG*` environment variables,
* it has no command that selects a model non-interactively.

The only interface of OpenCode that honours a model chosen on the command line
is the minimal one, `opencode mini --model <provider/model>`.

## Decision

We run `opencode mini --model <provider/model>` in the pseudo terminal, where
`<provider/model>` is the identifier the user picked in the model dialog. The
command is configurable through `terminal_command` in `config.toml`, so a user
who prefers another interface can replace it.

The automated pipeline is unaffected: it does not use the terminal at all, it
drives the agent through the Agent Client Protocol.

## Consequences

- **Easier:** The model the user picks in the dialog is the model the terminal
  runs, with no extra step and no reliance on a configuration file that the
  interface ignores. The command is one string, so it is easy to test and to
  replace.
- **Harder:** The minimal interface is less comfortable than the full one for
  long sessions. Users who want the full interface have to change
  `terminal_command` and accept that the model is then chosen inside the
  interface.
- **Given up:** The rich user interface of `opencode` by default.
- **Migration:** The key `opencode_command` was removed; `terminal_command`
  replaced it and carries the `--model` flag itself.

## Alternatives considered

- **Option A:** run the full `opencode` and let the user pick the model inside
  it. Rejected because the dialog would then promise a model that the
  terminal ignores.
- **Option B:** write a temporary `OPENCODE_CONFIG` file before the start.
  Rejected because the full interface ignores those variables.
- **Option C:** drive the manual session through ACP as well. Rejected because
  the user asked for a terminal: a full-screen, interactive program that can be
  interrupted, not a protocol session.
