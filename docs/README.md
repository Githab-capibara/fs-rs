# Documentation

| Section | Content |
|---------|---------|
| [Architecture Decision Records](adr/README.md) | The decisions that shape `fs-rs`, and the reasons behind them |
| [Design notes](design/README.md) | How the dashboard, the pipeline and the decision model are built |
| [Guides](guides/README.md) | Getting started, configuration, the local decision model |
| [Benchmarks](benchmarks/README.md) | The measured numbers of the local decision model |
| [Images](images/) | The architecture diagram and the charts of the reports |

## The project

`fs-rs` is a standalone Rust desktop dashboard that tracks the upstream commits
that a fork is missing, rates how critical and how complex they are with a
local model, and ports them into the fork with a three-agent pipeline.

| Layer | Crate | Content |
|-------|-------|---------|
| Domain | [`fsrs-core`](../crates/fsrs-core) | Repositories, upstream commits, ratings, runs, the store, the git operations |
| Decision | [`fsrs-ai`](../crates/fsrs-ai) | The client of the local Foq model, the schemas, the calibration, the accelerators |
| Agents | [`fsrs-acp`](../crates/fsrs-acp) | The Agent Client Protocol client, the prompts, the pipeline, the mock agent |
| Interface | [`fsrs-app`](../crates/fsrs-app) | The egui window, the background worker, the terminal panel |

## The documents

| Document | Purpose |
|----------|---------|
| [ADR-01](adr/ADR-01-horizontal-repository-strip.md) | Show the repositories in a horizontal strip |
| [ADR-02](adr/ADR-02-opencode-mini-for-the-manual-terminal.md) | Run `opencode mini` in the manual terminal |
| [ADR-03](adr/ADR-03-agents-only-through-acp.md) | Drive the agents only through the Agent Client Protocol |
| [ADR-04](adr/ADR-04-mock-agent-as-a-test-double.md) | Keep the mock agent in the library |
| [ADR-05](adr/ADR-05-foq-as-the-only-local-model.md) | Use Foq as the only local decision model |
| [ADR-06](adr/ADR-06-context-fitted-prompts.md) | Fit every prompt into the window of the runtime |
| [Design 01](design/01-dashboard-layout.md) | The layout of the window, the threads, the model picker |
| [Design 02](design/02-three-agent-pipeline.md) | The implement → review → verify cycle, the branch, the merge |
| [Design 03](design/03-local-decision-model.md) | The schemas, the prompt, the calibration, the window |
| [Getting started](guides/01-getting-started.md) | The first run of the dashboard |
| [Configuration](guides/02-configuration.md) | Every key of `config.toml` |
| [The local decision model](guides/03-local-decision-model.md) | The runtime, the GPU, the calibration |

## The verification of this documentation

Every number, every default and every link of this tree is checked against the
code by a script that lives in the repository:

```bash
python3 scripts/verify-docs.py
```

It compares the constants of the code with the text of the documents, checks
that the links resolve, and — when the decision model is running — compares
the window of the runtime with the one that the documents name.

## Continuous integration

[`docs`](../.github/workflows/ci.yml) — `.github/workflows/ci.yml` runs
rustfmt, clippy, the test suite, the verification of this documentation and
`cargo deny check` against [`deny.toml`](../deny.toml) on every push and pull
request.

| Job | What it runs |
|-----|--------------|
| Rustfmt | `cargo fmt --all --check` |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
| Tests | `cargo test --workspace` |
| Documentation | `python3 scripts/verify-docs.py` |
| cargo-deny | the licences, the advisories, the duplicate versions and the sources of the dependency tree |

A green build is the state the project is kept in: `cargo fmt`, `cargo clippy`
and `cargo test` are run before every commit, and the CI makes it public.
