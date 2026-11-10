# 03. The local decision model

- **Status:** Design note
- **Date:** 2026-11-09
- **Deciders:** @Githab-capibara
- **Researcher:** fs-rs
- **Purpose:** Explain how the criticality and the complexity of a commit are produced.
- **Feeds into:** [ADR-05](../adr/ADR-05-foq-as-the-only-local-model.md), [ADR-06](../adr/ADR-06-context-fitted-prompts.md)

## The task

The model answers exactly two questions about an upstream commit:

1. how **critical** is this commit for the fork?
2. how **complex** is the port of this commit into the fork?

Both questions are answered with one of five letters, and each letter carries a
description that is shown to the model:

| Letter | Criticality | Complexity |
|--------|-------------|------------|
| A | Critique — breaks the project, loses data, security hole | Expert — architectural rewrite |
| B | Élevée — major breakage that the user sees | Hard — risky cross-cutting change |
| C | Moyenne — bug fix or notable change of behaviour | Medium — several files, some design |
| D | Facile — local change, no design decision | Easy — local change, no design decision |
| E | Négligeable — trivial and isolated | Trivial — trivial and isolated |

The letters are mapped onto the domain types of `fsrs-core`: `Criticality` and
`Complexity`. The dashboard shows them as a badge — `C`, `H`, `M`, `L`, `T` for
the criticality and `T`, `E`, `M`, `H`, `X` for the complexity.

## The prompt

The prompt is byte-for-byte identical to the one of the reference Python
implementation of Foq. `crates/fsrs-ai/tests/prompt_conformance.rs` compares it
against a fixture that was produced by the reference, so a change of the prompt
that breaks the fine-tuning is caught by the test suite.

The context that is embedded in the prompt describes the commit passively:

```text
Commit: 39886c0cb34453cda21163df0c0c76fc9c011a21
Auteur: Thomas McCombs <tmccombs@users.noreply.github.com>
Date: 1762280000
Fichiers modifiés: 12 (+340 -58)
  - src/cli.rs
  - src/config.rs
Titre: Merge pull request #2219 from tmccombs/release-10.5.2
Description:
Release 10.5.2

Diff:
diff --git a/src/cli.rs b/src/cli.rs
...
```

The text is wrapped in a `<donnees>` block, and the four control markers that
could be used to escape the block are removed first.

## The answer and the calibration

The runtime returns, for the single generated token, the log-probability of
every candidate letter. The client takes the best log-probability of every
letter and turns them into a distribution with a softmax.

The raw logits of the model are systematically overconfident, so they are
calibrated with a temperature:

| Profile | Temperature | Runtime |
|---------|-------------|---------|
| `lora` | 2.6353 | the model with the decision adapter `foq_decision_8b.gguf` |
| `bare` | 3.6381 | the model alone |

The profile is explicit in the configuration (`foq_profile = "lora"`), because
the two temperatures are fitted for two different runtimes.

A decision whose calibrated confidence is below `min_confidence` (0.8 by
default) is flagged `needs_review`. The measured confidences of the benchmark
stay below that threshold, so in practice every rating is flagged: the model
orders the work, it does not decide it.

## The context window

The prompt has to fit into the window of the runtime. The window is not a
constant: it depends on how the runtime was started, and it is shared between
the slots of a parallel server.

The client therefore asks the runtime:

* `/props` → the size of the window, remembered after the first answer,
* `/tokenize` → the exact number of tokens of a prompt.

When the prompt does not fit, the context is cut from its end — the diff is
shortened first, the metadata of the commit survives — until the prompt fits.
The cut is marked with `… (contexte tronqué)`. The length of the longest
fitting prefix is found with a binary search, so the prompt uses as much of
the window as it can. See [ADR-06](../adr/ADR-06-context-fitted-prompts.md).

## The runtime

The model is served by the CUDA build of `llama-server` that ships with Foq:

```text
llama-server -m ~/.models/foq/foq-reflex-8b-pq2_0.gguf \
             --lora ~/.models/foq/foq_decision_8b.gguf \
             -ngl 99 -c 4096 -b 2048 -ub 512 -np 1 \
             --flash-attn on --port 8089 --host 127.0.0.1 \
             --alias foq,foq-system1
```

`-np 1` matters: with four slots the window of 4096 tokens is shared between
them and every slot only gets 1024 tokens. The dashboard runs at most two
ratings at a time and each rating sends its requests one after the other, so a
second slot buys nothing.

The service is a user unit, `foq-server.service`, and it is the only process
that holds the discrete GPU.

## The two requests

A rating is two requests: one for the criticality, one for the complexity. The
schema is the only difference between them. The calibration of the two
answers is independent.
