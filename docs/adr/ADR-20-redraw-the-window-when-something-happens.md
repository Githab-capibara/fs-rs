# 20. Redraw the window when something happens

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-19](ADR-19-draw-only-the-visible-rows.md),
  [`crates/fsrs-app/src/app.rs`](../../crates/fsrs-app/src/app.rs),
  [`crates/fsrs-app/tests/gui.rs`](../../crates/fsrs-app/tests/gui.rs)

## Context

The dashboard is a window that watches a background: the background fetches
the upstream branch, rates the commits, asks the agents and writes the log of
the pipeline. The window itself only shows what the background found.

A window that draws its frames in a loop keeps the processor of the machine
busy while the user reads the log of a pipeline that takes minutes. The frames
of such a window cost the reader nothing, because the picture does not change
between them, and they cost the pipeline the part of the processor that the
rating of the commits needs.

egui does not draw a frame on its own: it draws one when something asks for
it — a click of the user, an animation, or a request of the program. The
dashboard therefore has to ask for the frames that the background makes
necessary, and it has to ask for nothing else.

The question is what "something" means. The dashboard counted the tasks that
were in flight and asked for a frame when the number of them changed. A
message of the background that does not change the number of the tasks — the
number of the slots of the decision server, for example, or the list of the
models of the agent — changed the picture on the screen, and the window did
not draw it: the reader saw the old picture until something else happened.

## Decision

The window asks for a frame when the state of the window changed, and it
asks for nothing while the state stands still.

**Every message of the background is a change.** The handling of the messages
of the worker tells whether it applied at least one of them. A frame is asked
for when a message was applied, and also when the number of the tasks in
flight changed — the second case covers the tasks that the window starts on
its own, like the rating of a commit that the background reported.

**A busy window polls slowly.** While the window waits for the decision
server, for the agent or for the terminal, it asks for a frame after 250 ms.
The poll is a promise that the window will notice the answer of the server
even if the answer changes nothing that the window counts.

**A quiet window is quiet.** When nothing happens, the window asks for no
frame at all, and the processor of the machine is not spun. The test
`the_window_asks_for_a_frame_only_when_something_happens` of
[`crates/fsrs-app/tests/gui.rs`](../../crates/fsrs-app/tests/gui.rs) watches a
window whose decision model answers slowly: the window asks for frames while
the rating is in flight, and it asks for nothing once the rating is over.

## Consequences

- **Easier:** the cost of an idle window is zero. The dashboard can stay open
  for hours next to a long pipeline without taking the processor away from
  it. Every message of the background reaches the screen, and not only the
  ones that change the number of the tasks.
- **Harder:** the decision to draw a frame is made in one place, and a new
  source of change has to say that it changed something. The test of the
  window has to wait for a slow answer of the decision model, and it takes
  about six seconds.
- **Given up:** the window does not redraw itself when the data of a message
  is the same as the data that it already shows. A message that repeats the
  state that the window knows costs one frame.
- **Migration:** none. The change is inside the program, and the rows of the
  database hold the same data.

## Alternatives considered

- **Option A:** draw the frames in a loop, and keep the cost of a frame low.
  Rejected: the frame of the dashboard is 144–182 µs now, and a hundred
  frames a second would still take two percent of the processor of a core for
  a picture that does not change.
- **Option B:** count only the tasks that are in flight, and ask for a frame
  when the number of them changes. Rejected: a message that changes the
  picture without changing the number of the tasks would never reach the
  screen.
- **Option C:** ask for a frame from every thread that produces a message.
  Rejected: egui is not thread safe, and the request of a frame has to be
  made from the thread that draws.
