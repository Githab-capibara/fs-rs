# 01. Show the repositories in a horizontal strip

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** @Githab-capibara
- **Related:** [ADR-03](ADR-03-agents-only-through-acp.md), [design note](../design/01-dashboard-layout.md)

## Context

The dashboard tracks several forks at once. The user has to answer one
question before anything else: *which fork am I looking at, and how much work
is waiting in it?*

A vertical list of repositories was the first idea. It has two problems. It
pushes the commit table down, so the table — the part the user works in — sits
below the fold on a 1600×900 screen. And it spends a full column of width on a
piece of information that is one line tall.

The number of tracked repositories is small in practice: a handful of forks that
the user maintains. The list does not need the room that a full column takes.

## Decision

We render the repositories as a horizontal strip at the top of the window,
between the header and the commit table. Every repository occupies a framed
card that shows:

* the name of the repository,
* the path of the local working copy,
* the number of upstream commits that are still missing from the fork.

The selected repository is highlighted. The strip scrolls horizontally when the
repositories do not fit.

## Consequences

- **Easier:** The commit table starts directly under the strip, so the working
  area is visible without scrolling. The count of pending updates sits next to
  the path it belongs to, and the user compares forks without moving the
  pointer.
- **Harder:** A repository with a long name eats the width of its neighbours.
  The strip needs horizontal scrolling, and the cards need a minimum width.
- **Given up:** The vertical space of a repository list. A vertical list scales
  better with many repositories; the strip scales better with few.
- **Migration:** None. The layout is not persisted.

## Alternatives considered

- **Option A:** vertical sidebar of repositories. Rejected because it pushes
  the commit table below the fold and wastes the width of a column.
- **Option B:** a single `egui::ComboBox` that names the current repository.
  Rejected because the number of pending updates would be invisible unless the
  menu is open, and the user could not compare forks.
- **Option C:** a tab per repository. Rejected because the tabs hide the
  pending counts of the repositories that are not open.
