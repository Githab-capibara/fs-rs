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
| [`main`](src/main.rs) | The entry point: the arguments, the configuration, the database and the window. The allocator of the program is `mimalloc`, and the time to the first frame is logged |

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

The egui window, the terminal and the read of the database live on the UI
thread; everything else runs on a Tokio runtime with two workers. The store
is a `rusqlite` connection, which is not `Sync`, so the thread of the window
holds one connection and needs neither a lock nor a pool.

The background holds a connection of its own to the same database and writes
the commits, the ratings, the events of the run and the state of the run. A
write of SQLite waits for the disk, and the window of the dashboard must not
wait for the disk — the write of the worker is a message that the window
reads, not a pause of the window.

The worker holds one client of the local decision model and shares it with
every rating, so the connection to the server, the size of its window and the
prompts it already fitted are reused instead of asked again for every commit.
The client is replaced only when the settings of the user really change. The
number of the slots of the server bounds the number of the ratings that are
in flight: a request that waits in the queue of the server waits for the
model anyway.

The worker also keeps the statistics of the commits of every repository. The
statistics of a commit never change, so they are kept between two
synchronisations and handed to the next one: a fork that did not move is
listed without a single diff, and the database is not read for them.

## The cost of a frame

The table of the commits draws only the rows that are visible: the backlog of
a fork grows without a bound, and the window of the dashboard does not. The
rows live in a `egui::ScrollArea` with a fixed height of a row, and the
header of the columns shares one computation of the widths with the rows
themselves.

The state keeps what it derives — the rows of the commits, the assessments by
the object id of the commit, the indices of the models that match the filter
of the picker and the lines of the log — so that a frame does not rebuild
them.

The program times every part of every frame and writes the median, the
ninety-fifth percentile and the slowest frame of the last 512 frames to its
log. The environment variable `FSRS_CONTINUOUS_REPAINT` asks the window for a
frame after every one it has drawn, so that the measurement is of the steady
state:

```bash
scripts/measure-frames.sh target/release/fsrs /tmp/opencode/fd-fork 5
```

The numbers of that measurement are in
[the benchmarks](../../docs/benchmarks/README.md), and the decision behind
them is in
[ADR-19](../../docs/adr/ADR-19-draw-only-the-visible-rows.md).

## Related documents

| Document | Purpose |
|----------|---------|
| [ADR-01](../../docs/adr/ADR-01-horizontal-repository-strip.md) | Why the repositories are a horizontal strip |
| [ADR-02](../../docs/adr/ADR-02-opencode-mini-for-the-manual-terminal.md) | Why the manual terminal runs `opencode mini` |
| [ADR-10](../../docs/adr/ADR-10-the-decision-server-is-asked-four-times.md) | Why the dashboard asks the decision server four times per commit |
| [ADR-11](../../docs/adr/ADR-11-build-for-the-machine.md) | Why the build is tuned for the machine it runs on |
| [ADR-13](../../docs/adr/ADR-13-rate-the-commits-one-at-a-time.md) | Why the ratings never outnumber the slots of the decision server |
| [ADR-18](../../docs/adr/ADR-18-keep-the-statistics-of-the-commits-in-memory.md) | Why the worker keeps the statistics of the commits |
| [ADR-19](../../docs/adr/ADR-19-draw-only-the-visible-rows.md) | Why only the visible rows of the table are drawn |
| [Design note — the dashboard layout](../../docs/design/01-dashboard-layout.md) | The window, the threads, the picker, the screenshot |
| [Getting started](../../docs/guides/01-getting-started.md) | The first run of the dashboard |
