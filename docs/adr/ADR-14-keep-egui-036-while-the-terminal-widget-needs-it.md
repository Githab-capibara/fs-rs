# 14. Keep egui 0.36 while the terminal widget needs it

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-11](ADR-11-build-for-the-machine.md),
  [`crates/fsrs-app/Cargo.toml`](../../crates/fsrs-app/Cargo.toml)

## Context

`egui` 0.37.0 was published on 2026-10-15, and the checklist of the
performance work asks for the upgrade of the window to it.

The dashboard is not only a window: it holds the terminal panel, which is
drawn by `egui_tty` — a widget on top of the VT engine of Ghostty. The
versions of `egui_tty` that exist are:

| Version | Published | Requires |
|---------|-----------|----------|
| 0.1.0 | 2026-08-06 | `egui ^0.36` |
| 0.2.0 | 2026-08-07 | `egui ^0.36`, `eframe ^0.36` (for the tests) |

The last commit of the upstream of the widget — 2026-11-09 — still asks for
`egui = "0.36"`.

Two versions of `egui` cannot live in one program: `egui_tty` compiles
against the types of 0.36, and the window of the dashboard compiles against
the types of 0.37. The two are the same types with the same names, and the
compiler refuses to mix them.

## Decision

The window stays on `egui` 0.36 and `eframe` 0.36 until the terminal widget
is ready for 0.37.

The upgrade is not refused — it is waiting. It becomes possible in one of
three ways:

* `egui_tty` publishes a version that asks for `egui ^0.37`,
* the upstream of the widget accepts the change of the types of `egui` and
  publishes it,
* the terminal panel is drawn by the program itself, without the widget.

The first two are the work of the upstream of the widget; the third is the
work of this project, and it is the largest of the three.

## Consequences

- **Easier:** the program builds and runs with one version of the window, and
  the terminal panel keeps working. The upgrade is a single step when the
  widget is ready, not a change of the whole interface.
- **Harder:** the window does not have the improvements of 0.37 — the new
  APIs, the fixes of the rendering and the support of the new versions of the
  platforms. A bug of the window that was fixed upstream stays here.
- **Given up:** the upgrade of the window itself. The checklist asked for it,
  and the answer is that the dependency below it is not ready.
- **Migration:** none. The versions in `crates/fsrs-app/Cargo.toml` are the
  ones of the last release.

## Alternatives considered

- **Option A:** upgrade `egui` and `eframe` to 0.37 and keep `egui_tty`
  0.2.0. Rejected: the program does not compile — two versions of the same
  types in one program.
- **Option B:** upgrade `egui` to 0.37 and write the terminal panel on top of
  `libghostty-vt` directly, without the widget. Not rejected, but not done
  now: it is a new widget with its own tests, its own rendering and its own
  handling of the size of the cells, and the checklist of this round is about
  the speed, not about the terminal.
- **Option C:** replace the terminal with the one of the operating system.
  Rejected: the panel is a part of the window, and the program is built around
  it — the pipeline writes into it and the user reads the log of the run
  there.
