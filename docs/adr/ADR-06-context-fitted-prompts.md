# 06. Fit every prompt into the window of the runtime

- **Status:** Accepted
- **Date:** 2026-11-09
- **Deciders:** @Githab-capibara
- **Related:** [ADR-05](ADR-05-foq-as-the-only-local-model.md), [benchmark report](../benchmarks/README.md)

## Context

The rating of a commit puts the metadata of the commit, the list of the touched
files and a truncated patch into the prompt of the model. The size of that
prompt depends on the diff, and the runtime refuses a prompt that does not fit
into its context window:

```text
request (1380 tokens) exceeds the available context size (1024 tokens)
```

The window was smaller than expected. The runtime was started with `-c 4096`
and `-np 4`, and the context is shared between the slots: every slot got
`4096 / 4 = 1024` tokens. The failure appeared on the fifth commit of the
benchmark and aborted the whole run.

A fixed character budget cannot fix this. The number of tokens of a diff
depends on its content, and the window depends on how the runtime was started.

## Decision

We solve the problem on both sides.

**The runtime** is started with a single slot and the full window:

```text
-c 4096 -np 1
```

The dashboard never runs more than two ratings at a time, and each rating sends
its two requests one after the other, so a second slot buys nothing while it
takes a quarter of the window away from the diff.

**The client** learns the window from the runtime and fits every prompt into
it:

* `FoqClient::context_size` asks `/props` once and remembers the answer.
* `FoqClient::count_tokens` asks `/tokenize` for the exact number of tokens of
  a prompt.
* `FoqClient::fit_prompt` cuts the context down until the prompt fits. The
  context ends with the diff, so the diff is shortened first and the metadata
  of the commit survives. The cut is marked with `… (contexte tronqué)`.

The cut is searched with a binary search, so the prompt is as long as the
window allows. A runtime without a tokenizer falls back to a pessimistic
estimate of two characters per token.

The server is also asked to explain itself: a refusal that is not a broken
connection keeps the body of the answer, so the reason is visible in the log
of the dashboard instead of a bare status code.

## Consequences

- **Easier:** A long diff can no longer abort the run. The prompt always fits,
  whatever the runtime was started with. The user sees the reason of a failure
  instead of `http status: 400`.
- **Harder:** Every rating costs one extra request to `/tokenize`, and an
  oversized prompt costs a few more. The client depends on two endpoints of the
  runtime that are not part of the OpenAI-compatible API, and it degrades to an
  estimate when they are absent.
- **Given up:** The last part of a very large diff is not seen by the model.
  The rating of such a commit is based on its beginning, which is also what
  the model was trained on.
- **Migration:** The runtime has to be restarted with `-np 1`. The client needs
  no change; it adapts to the window that the runtime reports.

## Alternatives considered

- **Option A:** keep the runtime as it was and shorten the patch in the
  prompt builder. Rejected because the budget would depend on an assumption
  about the window that the runtime can change at any time.
- **Option B:** raise the window of the runtime to 16384 tokens. Rejected
  because the model itself was trained with 1024 tokens; a larger window
  increases the cost without improving the quality.
- **Option C:** send the diff in pieces and ask the model several times.
  Rejected because the decision schema expects one answer over the whole
  context.
