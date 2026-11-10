# 01. Dashboard layout

- **Status:** Design note
- **Date:** 2026-11-09
- **Deciders:** @Githab-capibara
- **Researcher:** fs-rs
- **Purpose:** Explain how the window of `fs-rs` is built and why every part is where it is.
- **Feeds into:** [ADR-01](../adr/ADR-01-horizontal-repository-strip.md), [ADR-02](../adr/ADR-02-opencode-mini-for-the-manual-terminal.md)

## The window

The window has four areas:

```text
┌────────────────────────────────────────────────────────────────────────┐
│ header — state of the dashboard, path of the configuration              │
├────────────────────────────────────────────────────────────────────────┤
│ repository strip — path and number of pending updates, side by side     │
├───────────────────────────────────────────────┬────────────────────────┤
│ table of the upstream commits                 │ terminal               │
│ ┌──────┬──────────────────────────┬────────┐ │ (OpenCode CLI)         │
│ │ crit │ description              │ complx │ │                        │
│ ├──────┼──────────────────────────┼────────┤ │                        │
│ │  M   │ fix: …                   │   E    │ │                        │
│ └──────┴──────────────────────────┴────────┘ │                        │
├───────────────────────────────────────────────┴────────────────────────┤
│ log — the events of the pipeline                                          │
└────────────────────────────────────────────────────────────────────────┘
```

The table has three columns plus the button: the criticality on the left, the
description in the middle, the complexity on the right, and `РЕАЛИЗОВАТЬ` at
the end of the row. The criticality and the complexity are shown as a letter
and a badge, because the space of a column is small and the badge carries the
colour.

## Threads

The interface must never block. Fetching an upstream repository takes
seconds, a rating of a commit takes seconds, and the pipeline takes minutes.
Therefore:

* the egui window, the store and the terminal live on the **UI thread**,
* everything else runs on a **Tokio runtime** with two worker threads,
* the background sends `WorkerMessage`s through a channel, and the interface
  drains the channel once per frame.

The store is a `rusqlite` connection, which is not `Sync`. Keeping it on the UI
thread avoids both a lock and a pool.

The rating queue is limited to two requests in flight, because the decision
model is a single process on a single GPU.

## The model picker

Clicking a commit — or its `РЕАЛИЗОВАТЬ` button — opens a modal dialog that
lists the models the agent exposes. The list comes from the configuration
option `model` of the ACP session; the choice is sent back with
`session/set_config_option`.

The dialog always renders the line `Выбрано: …`, with the placeholder
`не выбрано` when nothing is picked. This detail matters: when the line only
appeared after a choice, the dialog grew by 18 pixels, the confirm button
moved, and the click landed on the wrong widget. The line is always there, so
the layout never shifts under the pointer.

## The terminal

The terminal is a real pseudo terminal (`portable-pty`) rendered by the Ghostty
VT engine that `egui_tty` embeds. It is not a log view: the OpenCode CLI
behaves as it would in a terminal, with full-screen programs, colours and
interruptions.

The command is `opencode mini --model <provider/model>`, see
[ADR-02](../adr/ADR-02-opencode-mini-for-the-manual-terminal.md).

## The screenshot of the documentation

The picture in the README is not a mock-up: it is the real window, rendered by
the real code. `crates/fsrs-app/examples/screenshot.rs` builds the window
through `egui_kittest`, fills it with prepared rows and saves the frame:

```bash
cargo run -p fsrs-app --example screenshot -- docs/images/hero.png
```

`FsrsApp::prepare_screenshot` puts the rows into the window and switches it
into the prepared mode, in which the messages of the background worker are
ignored — otherwise a failed synchronisation would empty the picture again.

## What the tests cover

`crates/fsrs-app/tests/gui.rs` drives the real window through `egui_kittest`:

* the repository strip and the table show the tracked repository and its
  missing commits,
* clicking a commit opens the model picker,
* `РЕАЛИЗОВАТЬ` runs the whole pipeline against the mock agent and merges the
  result into `main`,
* the picker offers the models of the mock agent,
* the commits are rated by a stand-in for the decision model,
* the window survives an agent that cannot be started.

The tests use `harness.step()` and never `harness.run()`: the app asks for a
repaint, and `run()` stops after four steps with a panic.

## The language of the window

The window speaks Russian, and the Russian language has three plural forms:
`1 файл`, `2 файла` and `5 файлов`. `state::plural` chooses the form from the
number — the last digit, except for the numbers that end in 11, 12, 13 and 14.
Every count in the window goes through it: the number of the updates, the
number of the files, the number of the commits that wait for their assessment
and the number of the rework cycles.
