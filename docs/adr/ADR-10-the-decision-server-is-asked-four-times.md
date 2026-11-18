# 10. Ask the decision server four times, not thirty

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-06](ADR-06-context-fitted-prompts.md),
  [Design note — the decision model](../design/03-local-decision-model.md),
  [`crates/fsrs-ai/benches/context.rs`](../../crates/fsrs-ai/benches/context.rs)

## Context

The dashboard rates every commit of the backlog of a fork: the local model
judges how **critical** the commit is and how **complex** the port of it
would be. The model runs on the same machine, over the loopback, and every
question costs seconds.

The client asked the server far more often than the questions needed:

1. Every request built a new `ureq::Agent`, so every question opened a new
   connection and paid for a handshake.
2. The size of the context window was asked with `/props` by every client,
   and the dashboard built a new client for every commit.
3. The fitting of the prompt asked `/tokenize` once per step of a binary
   search over the length of the cut — more than a dozen times for a large
   diff, and twice, because a commit is rated by two schemas.
4. Every request carried its own timeout, so a server that was slow held the
   connection for the whole timeout of the longest question.

## Decision

One client is shared by every rating of the dashboard, and it is asked as
little as possible:

* the client holds one `ureq::Agent`, created once and reused, so the
  connection to the server stays open between the questions,
* the size of the window is asked once per client and remembered
  (`OnceLock`),
* the fitted context is remembered by the hash of its text, so the second
  schema of a commit reads the same context without asking the server to
  count the tokens again,
* the fitting counts the tokens of the whole context once, estimates the
  length of the prefix that fits from that count, and corrects the estimate —
  it does not search the length of the cut,
* every request carries its own timeout: two seconds for the health, five
  for the properties, ten for the tokenizer, and the timeout of the
  configuration for the completion.

The body of a completion is written and read with `sonic-rs`; the small
untyped answers of `/props` and `/tokenize` stay with `serde_json`.

## Consequences

- **Easier:** one rating of a commit asks the server four times — the window,
  the count of the tokens, and two completions — instead of six. A series of
  three commits asks the server ten times instead of twelve, and takes 9.4 s
  instead of 12.4 s. The fitting of a prompt that does not fit asks the server
  ten times instead of fourteen.
- **Harder:** the client holds a cache of the fitted contexts, so it has to
  be bounded — the cache is per client, and a client lives as long as the
  dashboard.
- **Given up:** the exact longest prefix that fits is not found. The estimate
  is corrected until the prompt fits or the cut cannot be changed, which
  costs a few calls and can leave a small amount of the window unused.
- **Migration:** none. The client is built the same way; only the number of
  requests it makes changed.

## Alternatives considered

- **Option A:** keep the binary search of the cut and cache the counts of the
  prefixes. Rejected because the cache grows with the length of the context
  and the search still asks the server for the counts it does not have.
- **Option B:** count the tokens locally instead of asking the server. Rejected
  because the tokenizer of the model is the only exact one, and a local
  approximation would silently change the prompts that are sent.
