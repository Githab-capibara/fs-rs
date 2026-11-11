# `fsrs-ai`

The client of the local decision model. Foq is a decision engine, not a chat
model: it answers with a letter and returns the log-probability of every
letter, which this crate turns into a calibrated confidence.

The model does nothing else. It never writes code and never talks to the
agents — it only answers the two questions about a commit: how critical it is
and how complex its port is.

| Module | Content |
|--------|---------|
| [`schema`](src/schema.rs) | The two decision schemas: `criticality` and `complexity`, five lettered options each, and the prompt layout of the reference implementation |
| [`prompt`](src/prompt.rs) | The context of a commit: the metadata, the list of the touched files and the truncated diff |
| [`calibration`](src/calibration.rs) | The temperature scaling of the raw log-probabilities: `T = 2.6353` for the model with the adapter, `T = 3.6381` for the bare model |
| [`client`](src/client.rs) | The HTTP client: the request, the answer, the fitting of the prompt into the window of the runtime, and the body of a refused request |
| [`gpu`](src/gpu.rs) | The accelerators of the machine: the NVIDIA devices, the Intel devices and the processes that hold the GPU |

| Tool | What it does |
|------|--------------|
| [`examples/benchmark.rs`](examples/benchmark.rs) | Rates the commits of a repository and writes the report as JSON |

```bash
cargo run -p fsrs-ai --example benchmark -- /path/to/upstream HEAD 60 \
    > report.json
```

| Test suite | What it covers |
|------------|----------------|
| [`tests/prompt_conformance.rs`](tests/prompt_conformance.rs) | The prompt against the fixture of the reference Python implementation — a drift would silently degrade every decision |
| [`tests/live_server.rs`](tests/live_server.rs) | The client against a running Foq server: the answers, the context fitting, the refused request. The tests skip themselves when no server listens |

## Related documents

| Document | Purpose |
|----------|---------|
| [ADR-05](../../docs/adr/ADR-05-foq-as-the-only-local-model.md) | Why Foq, and why it is the only model |
| [ADR-06](../../docs/adr/ADR-06-context-fitted-prompts.md) | How the prompt is fitted into the window of the runtime |
| [Design note — the decision model](../../docs/design/03-local-decision-model.md) | The schemas, the prompt, the calibration |
| [Guide — the local decision model](../../docs/guides/03-local-decision-model.md) | The runtime, the GPU and the calibration profiles |
| [Benchmark report](../../docs/benchmarks/README.md) | The measured numbers of 60 real commits |
