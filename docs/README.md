# Documentation

| Section | Content |
|---------|---------|
| [Architecture Decision Records](adr/README.md) | The decisions that shape `fs-rs`, and the reasons behind them |
| [Design notes](design/README.md) | How the dashboard, the pipeline and the decision model are built |
| [Guides](guides/README.md) | Getting started, configuration, the local decision model |
| [Benchmarks](benchmarks/README.md) | The measured numbers of the local decision model, of the synchronisation and of the frames |
| [Images](images/) | The architecture diagram and the charts of the reports |

## The project

`fs-rs` is a standalone Rust desktop dashboard that tracks the upstream commits
that a fork is missing, rates how critical and how complex they are with a
local model, and ports them into the fork with a three-agent pipeline.

| Layer | Crate | Content |
|-------|-------|---------|
| Domain | [`fsrs-core`](../crates/fsrs-core/README.md) | Repositories, upstream commits, ratings, runs, the store, the git operations |
| Decision | [`fsrs-ai`](../crates/fsrs-ai/README.md) | The client of the local Foq model, the schemas, the calibration, the accelerators |
| Agents | [`fsrs-acp`](../crates/fsrs-acp/README.md) | The Agent Client Protocol client, the prompts, the pipeline, the mock agent |
| Interface | [`fsrs-app`](../crates/fsrs-app/README.md) | The egui window, the background worker, the terminal panel |

## The documents

| Document | Purpose |
|----------|---------|
| [ADR-01](adr/ADR-01-horizontal-repository-strip.md) | Show the repositories in a horizontal strip |
| [ADR-02](adr/ADR-02-opencode-mini-for-the-manual-terminal.md) | Run `opencode mini` in the manual terminal |
| [ADR-03](adr/ADR-03-agents-only-through-acp.md) | Drive the agents only through the Agent Client Protocol |
| [ADR-04](adr/ADR-04-mock-agent-as-a-test-double.md) | Keep the mock agent in the library |
| [ADR-05](adr/ADR-05-foq-as-the-only-local-model.md) | Use Foq as the only local decision model |
| [ADR-06](adr/ADR-06-context-fitted-prompts.md) | Fit every prompt into the window of the runtime |
| [ADR-09](adr/ADR-09-commits-are-counted-once.md) | Count every commit once |
| [ADR-10](adr/ADR-10-the-decision-server-is-asked-four-times.md) | Ask the decision server four times, not thirty |
| [ADR-11](adr/ADR-11-build-for-the-machine.md) | Build the dashboard for the machine it runs on |
| [ADR-12](adr/ADR-12-read-the-objects-as-fast-as-git-can.md) | Read the objects of a fork as fast as git can read them |
| [ADR-13](adr/ADR-13-rate-the-commits-one-at-a-time.md) | Rate the commits one at a time |
| [ADR-14](adr/ADR-14-keep-egui-036-while-the-terminal-widget-needs-it.md) | Keep egui 0.36 while the terminal widget needs it |
| [ADR-15](adr/ADR-15-lay-the-code-out-again-with-bolt.md) | Lay the code out again with BOLT |
| [ADR-16](adr/ADR-16-count-the-lines-of-a-commit-in-one-pass.md) | Count the lines of a commit in one pass |
| [ADR-17](adr/ADR-17-write-only-the-rows-that-really-changed.md) | Write only the rows that really changed |
| [ADR-18](adr/ADR-18-keep-the-statistics-of-the-commits-in-memory.md) | Keep the statistics of the commits in memory |
| [ADR-19](adr/ADR-19-draw-only-the-visible-rows.md) | Draw only the visible rows of the table of the commits |
| [ADR-20](adr/ADR-20-redraw-the-window-when-something-happens.md) | Redraw the window when something happens |
| [ADR-21](adr/ADR-21-write-the-events-of-a-run-in-one-transaction.md) | Write the events of a run in one transaction |
| [ADR-22](adr/ADR-22-write-the-big-bodies-with-sonic-rs.md) | Write the big bodies with sonic-rs |
| [Design 01](design/01-dashboard-layout.md) | The layout of the window, the threads, the model picker |
| [Design 02](design/02-three-agent-pipeline.md) | The implement → review → verify cycle, the branch, the merge |
| [Design 03](design/03-local-decision-model.md) | The schemas, the prompt, the calibration, the window |
| [Getting started](guides/01-getting-started.md) | The first run of the dashboard |
| [Configuration](guides/02-configuration.md) | Every key of `config.toml` |
| [The local decision model](guides/03-local-decision-model.md) | The runtime, the GPU, the calibration |
| [Benchmarks](benchmarks/README.md) | The decision model, the synchronisation and the frames of the window, measured |

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

The jobs that compile the workspace install zig 0.15.2 first, because the
terminal panel renders its output with the Ghostty VT engine, which
`libghostty-vt-sys` builds with `zig build`.

A green build is the state the project is kept in: `cargo fmt`, `cargo clippy`
and `cargo test` are run before every commit, and the CI makes it public.
