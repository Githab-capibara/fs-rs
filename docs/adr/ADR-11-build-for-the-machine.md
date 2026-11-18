# 11. Build the dashboard for the machine it runs on

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-10](ADR-10-the-decision-server-is-asked-four-times.md),
  [`scripts/build-pgo.sh`](../../scripts/build-pgo.sh)

## Context

The dashboard is a desktop program: it is started by the user, it draws a
window and it answers the clicks. The time from the click to the first frame
is what the user waits for, and the size of the binary is what they download
and what they keep in memory.

The build was tuned for portability, not for speed:

* the release profile linked the crates separately (`lto = "thin"`), so the
  calls between them went through a table,
* the window allocated and freed thousands of small blocks per frame with
  the allocator of the system,
* the compiler knew nothing about the code the program actually runs.

## Decision

The build is tuned for the machine it runs on:

* `.cargo/config.toml` sets `-C target-cpu=native`, so the mathematics of the
  window and the JSON library use the instructions of the processor,
* the release profile links the whole program as one crate (`lto = "fat"`),
* the allocator of the program is `mimalloc`, which is faster than the one of
  the system for the many small blocks of a frame,
* the build with a profile (`scripts/build-pgo.sh`) runs the instrumented
  program, gathers the profile of the real workload and lets the compiler lay
  the hot paths out first.

The tracing of the release builds stops at the level `info`, so the debug
records of the hot paths do not cost anything.

## Consequences

- **Easier:** the program starts in 3.3 ms instead of 3.4 ms, and the binary
  is 14.7 MiB instead of 14.9 MiB — the profile-guided build is a little
  smaller and a little less code. The numbers are the medians of forty
  interleaved runs of `scripts/measure-startup.py`.
- **Harder:** the binary of a build with `target-cpu=native` runs only on a
  processor with the same instructions. A build for a distribution has to
  override the flag.
- **Given up:** the profile-guided build depends on a run of the program: the
  profile of a run that only reads the configuration is not the profile of a
  run that rates a commit. The build script therefore takes the training
  command as an argument.
- **Migration:** none. The ordinary `cargo build --release` still works and
  is still the default.

## Alternatives considered

- **Option A:** keep the portable build and ship two binaries. Rejected
  because the dashboard is not distributed — it is built and run on the same
  machine.
- **Option B:** use `llvm-bolt` for the layout of the hot paths. Tried later
  and measured — see [ADR-15](ADR-15-lay-the-code-out-again-with-bolt.md).
  The tool works, and the measurement says that the time of the program is
  not in the code it moves, so the build with a profile of the compiler stays
  the way the program is built.
