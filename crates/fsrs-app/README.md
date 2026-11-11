# `fsrs-app`

The window of the dashboard: the strip of the repositories, the table of the
missing commits, the model picker, the terminal panel and the log of the
pipeline. The interface is egui, the terminal is a real pseudo terminal
rendered by the Ghostty VT engine.

| Module | Content |
|--------|---------|
| [`app`](src/app.rs) | The window itself: the layout, the actions, the model picker, the handling of the messages of the background |
| [`state`](src/state.rs) | The view types: `RepoView`, `ModelPicker`, `PipelineLog`, the badges, the colours and the Russian plural forms |
| [`worker`](src/worker.rs) | The background executor: the synchronisation, the ratings, the discovery of the models and the pipeline, all off the UI thread |
| [`terminal`](src/terminal.rs) | The terminal panel: the pseudo terminal, the reader thread and the termination of the child process |
| [`main`](src/main.rs) | The entry point: the arguments, the configuration, the database and the window |

| Binary | What it is |
|--------|------------|
| `fsrs` | The dashboard itself |
| [`src/bin/fsrs-mock-agent.rs`](src/bin/fsrs-mock-agent.rs) | The mock agent as a process, so that the dashboard tests can start an agent |

| Tool | What it does |
|------|--------------|
| [`examples/screenshot.rs`](examples/screenshot.rs) | Renders the real window into an image, for the documentation |

```bash
cargo run -p fsrs-app --example screenshot -- docs/images/hero.png
```

| Test suite | What it covers |
|------------|----------------|
| [`tests/gui.rs`](tests/gui.rs) | The window through `egui_kittest`: the strip, the table, the picker, the whole pipeline against the mock agent, and the survival of a missing agent |

## The threads

The egui window, the store and the terminal live on the UI thread; everything
else runs on a Tokio runtime with two workers. The store is a `rusqlite`
connection, which is not `Sync`, so it stays on the UI thread and needs
neither a lock nor a pool.

## Related documents

| Document | Purpose |
|----------|---------|
| [ADR-01](../../docs/adr/ADR-01-horizontal-repository-strip.md) | Why the repositories are a horizontal strip |
| [ADR-02](../../docs/adr/ADR-02-opencode-mini-for-the-manual-terminal.md) | Why the manual terminal runs `opencode mini` |
| [Design note — the dashboard layout](../../docs/design/01-dashboard-layout.md) | The window, the threads, the picker, the screenshot |
| [Getting started](../../docs/guides/01-getting-started.md) | The first run of the dashboard |
