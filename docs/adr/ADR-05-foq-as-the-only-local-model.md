# 05. Use Foq as the only local decision model

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** @Githab-capibara
- **Related:** [ADR-06](ADR-06-context-fitted-prompts.md), [benchmark report](../benchmarks/README.md)

## Context

The dashboard has to rate two things for every upstream commit: how critical
the commit is for the fork, and how complex the port of that commit is. Those
ratings drive the order of the work.

The ratings must be produced by a model that runs on the machine of the user:
the commits of a private fork are not allowed to leave it, and the dashboard
has to work without an account.

Several local models were available on the machine. The retired
`OxCoder-9B IQ4_XS` (`local/oxcoder-9b`) was explicitly forbidden.

Foq was chosen because it fits the task exactly:

* it is a decision engine, not a chat model — it answers with a letter and the
  log-probability of every letter,
* it ships a fine-tuned decision adapter (`foq_decision_8b.gguf`) and a
  calibration profile fitted for it,
* it is MIT licensed, and the weights (`Yoedi/foq-reflex-8b-gguf`) are
  Apache-2.0,
* it runs on the discrete GPU of the machine through the CUDA build of
  `llama-server`.

## Decision

We use Foq as the only local model, and only for the two ratings. It never
writes code, never talks to the agents and never takes part in the pipeline.

The rating is produced by two schemas, `criticality` and `complexity`, each
with five lettered options. The per-letter log-probabilities are calibrated
with temperature scaling: `T = 2.6353` for the model with the adapter, `T =
3.6381` for the bare model. The profile is explicit in the configuration
(`foq_profile = "lora"`).

The retired build stays forbidden: `fsrs-acp` filters `local/oxcoder-9b` out
of the list of models that the user can pick, so it can never be selected for
an agent.

## Consequences

- **Easier:** The ratings are reproducible, local and free. The prompt is
  byte-for-byte identical to the reference Python implementation, so the
  model behaves exactly as it was fine-tuned to. The calibration profile turns
  the raw log-probabilities into a confidence that can be compared between
  commits.
- **Harder:** The model needs a GPU to be useful, and the GPU must not be held
  by an unrelated service. A single decision takes seconds, not milliseconds,
  so the dashboard keeps at most two ratings in flight.
- **Given up:** The ratings are coarse. The measured confidences stay below
  the default threshold of 0.8, so in practice every rating is flagged for
  review — the model orders the work, it does not decide it.
- **Migration:** None.

## Alternatives considered

- **Option A:** the retired `OxCoder-9B IQ4_XS`. Rejected: it is forbidden.
- **Option B:** a cloud model. Rejected because the commits of a private fork
  must not leave the machine.
- **Option C:** a heuristic instead of a model — for example, the number of
  touched files. Rejected because the rating of a commit is a judgement, and
  the whole point of the decision adapter is that it was trained for exactly
  this judgement.
