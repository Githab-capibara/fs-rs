# Design Documents

Notes that explain how a part of `fs-rs` is built. An ADR records a *decision*;
a design note records the *reasoning and the shape* behind it.

| Document | Purpose |
|----------|---------|
| [01 — Dashboard layout](01-dashboard-layout.md) | How the window is built and why |
| [02 — The three-agent pipeline](02-three-agent-pipeline.md) | How a commit is ported and verified |
| [03 — The local decision model](03-local-decision-model.md) | How a commit is rated |

## Status of the project

| Stage | State |
|-------|-------|
| Domain, git and storage | Done, covered by unit and integration tests |
| Local decision model | Done, covered by unit tests, conformance tests and live tests |
| ACP client and pipeline | Done, covered by mock tests and live tests against `opencode acp` |
| Dashboard | Done, covered by interaction tests through `egui_kittest` |
| Documentation | This tree |
