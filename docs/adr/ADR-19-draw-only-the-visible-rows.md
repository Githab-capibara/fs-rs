# 19. Draw only the visible rows of the table of the commits

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-11](ADR-11-build-for-the-machine.md),
  [ADR-18](ADR-18-keep-the-statistics-of-the-commits-in-memory.md),
  [`crates/fsrs-app/src/app.rs`](../../crates/fsrs-app/src/app.rs),
  [`crates/fsrs-app/src/state.rs`](../../crates/fsrs-app/src/state.rs),
  [`crates/fsrs-app/src/worker.rs`](../../crates/fsrs-app/src/worker.rs)

## Context

The dashboard draws a frame again and again while the pipeline runs, and the
table of the commits is the part of the window that grows with the size of the
backlog. A fork of the `fd` project that is 1450 commits behind its upstream
has 1450 rows to draw.

Every row was drawn in every frame, and egui lays out and paints every widget
that is handed to it: the row of a commit that is far below the fold cost the
same as the row that the reader looks at. The measurement against the real
fork showed a median frame of **13.5–14.0 ms**, of which **13.4–13.9 ms**
were the table of the commits — the other parts of the window together cost
about 100 µs. A frame of fourteen milliseconds cannot be drawn at sixty frames
a second, and the window of the dashboard lagged behind the pipeline.

After the change the same measurement gives a median frame of **144–182 µs**,
of which **70–86 µs** are the table of the commits: the frame is **about
eighty times faster**, and the table of it **about a hundred and seventy
times faster**. The cost of a frame no longer depends on the size of the
backlog.

The cost of a frame was not only the drawing. Every frame copied the texts of
the header, the names of the repositories and the lines of the log, and it
rebuilt the list of the models that match the filter of the picker. The
interface also wrote the ratings, the events of the run and the state of the
run into the database — the writes of SQLite are synchronous, and they were
paid by the thread that draws the window.

## Decision

The table of the commits draws only the rows that are visible, and the data
that the window shows is computed once and kept.

**The table is virtualised.** The rows live in a `egui::ScrollArea` that is
asked for the rows of the window alone — `ScrollArea::show_rows` with a fixed
height of a row. The height of a row is a constant, so the scroll area knows
where every row is without asking for its content, and the rows outside of
the window are never laid out. The names of the columns are drawn in a header
of their own, and the header and the rows share one computation of the widths
of the columns, so that the names stand above the very columns they name.

**The state keeps what it derives.** `RepoView` holds the rows of the commits
it shows — the description, the numbers of the lines and the list of the files
— and the assessments by the object id of the commit, so that a row is found
without a search through the list. `ModelPicker` keeps the indices of the
models that match its filter and hands them out as a shared slice, until the
filter changes. `PipelineLog` keeps the lines it shows ready to be drawn.

**The database is written by the background.** The worker owns a connection of
its own to the store and writes the commits, the ratings, the events of the run
and the state of the run. The interface reads the state of the run and sends
the work to the worker; it never waits for a write.

**The frames are measured.** The program times every part of every frame and
writes the median, the ninety-fifth percentile and the slowest frame of the
last 512 frames to its log, together with the time of each part. The
environment variable `FSRS_CONTINUOUS_REPAINT` makes the window draw frames
that follow each other, so that the measurement is of the steady state and not
of the frames that the pipeline happened to cause. The script
[`scripts/measure-frames.sh`](../../scripts/measure-frames.sh) opens the
dashboard against a real fork and reads the numbers out of its log.

## Consequences

- **Easier:** the cost of a frame no longer depends on the size of the
  backlog. The table of a fork that is behind by a thousand commits costs the
  same as the table of a fork that is up to date. The reader sees the window
  move while the pipeline runs.
- **Harder:** the rows of the table have a fixed height, and a commit whose
  description needs more than one line is cut. The widths of the columns are
  computed by one function, and a new column has to be added to it and to the
  header at the same time.
- **Given up:** the window no longer writes to the database by itself, and a
  row of the database is a little later — the ratings of a commit appear in
  the database after the worker wrote them, not in the frame that received the
  rating.
- **Migration:** none. The rows of the database hold the same data, and the
  program reads them the same way.

## Alternatives considered

- **Option A:** draw every row, and make the drawing of a row cheaper.
  Rejected: the work of a row that is not visible is work nobody sees, and the
  backlog of a fork grows without a bound, while the window of the dashboard
  does not.
- **Option B:** draw the rows in pages, and load the next page when the reader
  scrolls. Rejected: the rows are already in memory — the worker lists them
  all — and the pagination would add a state that the scroll area does not
  need.
- **Option C:** keep the rows of the table in the database and read them when
  they are visible. Rejected: the database is the place where the state
  survives a restart, not the place where the state of a running program is
  read from, and ADR-18 moved the statistics of the commits into the memory
  of the worker for exactly this reason.
- **Option D:** keep the writes of the database on the thread of the window.
  Rejected: the write of SQLite waits for the disk, and the window waited for
  the disk with it.
