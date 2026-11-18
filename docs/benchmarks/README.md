# Benchmark of the local decision model

The numbers below come from a real run of the local model against the upstream
of a real project. The raw report is [`decision-model.json`](decision-model.json).

## The run

| Parameter | Value |
|-----------|-------|
| Repository | `sharkdp/fd`, shallow clone, 1451 commits |
| Revision | `HEAD` |
| Commits rated | 60, newest first |
| Model | `foq` — Foq 8B with the decision adapter, CUDA on the Quadro M4000M |
| Window | 4096 tokens, one slot |
| Failures | 0 |

```bash
cargo run -p fsrs-ai --example benchmark -- <repository> HEAD 60 \
    > docs/benchmarks/decision-model.json
```

## The distribution

![Distribution of the criticality and the complexity](../images/benchmark-distribution.svg)

| Criticality | Commits | Share |
|-------------|---------|-------|
| High | 2 | 3% |
| Medium | 40 | 67% |
| Low | 16 | 27% |
| Trivial | 2 | 3% |

| Complexity | Commits | Share |
|------------|---------|-------|
| Medium | 15 | 25% |
| Easy | 45 | 75% |

The 60 newest commits of `fd` are release commits, merges of pull requests and
dependabot updates. The model puts almost all of them in the middle of the
scale, which matches the work: a release or a dependency bump is not trivial,
but it is rarely a rupture of the project.

## The latency

Every rating is two requests — one for the criticality, one for the complexity
— on a Quadro M4000M with a quantised 8B model.

| Percentile | Time |
|------------|------|
| Minimum | 4.9 s |
| p50 | 8.9 s |
| Mean | 11.5 s |
| p95 | 26.6 s |
| Maximum | 34.9 s |

The spread is the size of the diff: a small commit is rated in a few seconds,
a large one takes half a minute, because the whole diff has to be read by the
model.

The dashboard keeps at most two ratings in flight, because the model is a
single process on a single GPU. The 60 commits of this run would therefore
take about six minutes in the dashboard.

## The confidence

| Metric | Minimum | Median | Maximum |
|--------|---------|--------|---------|
| Criticality | 0.301 | 0.464 | 0.739 |
| Complexity | 0.317 | 0.478 | 0.739 |

**No rating reached the default threshold of 0.8.** Every one of the 60
commits was flagged `needs_review`.

This is the expected behaviour of a calibrated model, not a defect: the
temperature of the `lora` profile (2.6353) flattens the distribution, so the
model rarely claims to be certain. The practical consequence is that the
ratings are an **ordering** of the work, not a decision about it. A commit that
the model rated `Medium` deserves to be looked at before one that it rated
`Low`, and the human still decides.

The threshold is configurable:

```toml
min_confidence = 0.45
```

With that value, roughly half of the ratings of this run would be accepted
without a review.

# Benchmark of the binary

The size of the program is a measurement like any other: a dashboard that
grows by a megabyte after every change is a dashboard nobody wants to
start. Two tools measure it, and they measure different things:

```bash
cargo build --release
ls -l target/release/fsrs
size target/release/fsrs

cargo bloat --release --crates --bin fsrs -p fsrs-app
```

The release profile strips the binary, so the file that the user runs is the
small one. `cargo-bloat` builds the program again with the debug information,
because it needs the symbols to say which crate a piece of code belongs to —
its file size is therefore the size of the binary with the symbols in it.

## The size

| Measurement | Value |
|-------------|-------|
| The stripped file | 14.9 MiB (15 672 400 B) |
| The section of the code of the stripped file | 14.6 MiB (15 259 252 B) |
| The file with the symbols, as `cargo-bloat` builds it | 26.6 MiB |
| The code that `cargo-bloat` attributes to the crates | 10.9 MiB |
| The crates in it | 154 |

The biggest parts of the code are the ones the dashboard cannot do without:
the window (`egui`, `winit`, `epaint`), the protocol of the agents
(`agent_client_protocol`), the fonts (`read_fonts`, `skrifa`) and the
terminal (`libghostty_vt_sys`). The code of the dashboard itself is small:
141.5 KiB, one and a third percent of the code.

## The profile-guided optimisation

The build with a profile measures the program while it runs and lets the
compiler lay the hot paths out first:

```bash
FSRS_PGO_REPOSITORY=/path/to/repo FSRS_PGO_GIT_REPOSITORY=/path/to/fork \
    scripts/build-pgo.sh
```

The tool that merges the profiles has to be the one of the same toolchain as
the compiler — the format of the raw profiles changes between the versions:

```bash
rustup component add llvm-tools-preview
```

The profile is gathered by running the program, so the numbers depend on the
workload: the profile of a run that only reads the configuration is not the
profile of a run that rates a commit. The default training of the script
rates the commits of a real repository with the local model and lists the
missing commits of a real fork — the two things the dashboard spends its
time on.

## The start of the program

The startup is the time the machine needs before the program does anything:
the loading of the binary, the dynamic linker and the runtime of rust. The
script that measures it runs the binary with `--help` — it reads its
arguments, writes its help and exits — and it runs the binaries one after
another, so the machine is in the same state for every one of them, and
reports the median:

```bash
python3 scripts/measure-startup.py target/release/fsrs target/pgo/release/fsrs
```

| Binary | The start of the process |
|--------|---------------------------|
| The ordinary build | 3364 µs |
| The build with the profile | 3312 µs |

The numbers are the medians of forty runs of each binary. The difference is
inside the noise: the start of the program is the loading of the binary and
the reading of the configuration, and neither of them is hot code.

The first frame of the window is a different measurement — it is the creation
of the surface and the first draw of the program — and the program logs it
when it happens: 60 ms to 180 ms on this machine, depending on the compositor.

## The layout of the code again — BOLT

The fortieth point of the checklist asks for the binary to be laid out again
by BOLT — the tool that moves the hot functions next to each other and splits
the cold code away from the hot one. The tool is not in the toolchain of
rust, but it was tried: the one of the Arch archive (23.1.5) reads the binary
of the compiler (LLVM 22.1.8) and writes a new one that runs.

The profile of the run in the format of BOLT is recorded from the dashboard
itself, against a real fork — the `fd` project, 1450 commits behind its
upstream:

```bash
scripts/record-bolt-profile.sh target/pgo/release/fsrs
FSRS_PGO_BOLT_DATA=target/pgo/bolt.fdata scripts/build-pgo.sh --bolt
```

The profile of that run holds 143600 samples and 4594907 branch entries, and
it covers 556 of the 10768 functions of the binary. BOLT shortened 32552
instructions, removed 776 empty blocks, folded 9 functions into the ones they
do the same thing as, and moved the hot blocks of 481 functions to the front
of their function.

The result, measured the same way as the other two:

| Binary | The start of the program | The listing of the commits |
|--------|--------------------------|---------------------------|
| The ordinary build | 3364 µs | 1245 ms |
| The build with the profile | 3312 µs | 1254 ms |
| The build with the profile and BOLT | 3362 µs | 1223 ms |

The differences are inside the noise of the machine. The reason is in the
profile: the dashboard spends its time in the code of `libgit2` — the
comparison of the trees of every commit — and in the model that answers the
questions, and neither of them is the code that BOLT moves around.

The listing of the commits is measured by the script that opens the
dashboard against a real fork and reads the time out of its log:

```bash
scripts/measure-sync.sh target/release/fsrs /tmp/opencode/fd-fork master 5
```

See [ADR-15](../adr/ADR-15-lay-the-code-out-again-with-bolt.md).

## What the run proves

* Every commit was rated — 0 failures. Before
  [ADR-06](../adr/ADR-06-context-fitted-prompts.md), the fifth commit
  aborted the whole run with `http status: 400`.
* A commit that the model refuses to rate no longer stops the run: the
  benchmark records the failure in the report and continues.
* The context of the commit is shortened instead of the prompt being rejected:
  the longest prompt of the run still fitted into the window.

## Reproducing

```bash
git clone --depth 400 https://github.com/sharkdp/fd.git /tmp/bench-upstream
cargo run -p fsrs-ai --example benchmark -- /tmp/bench-upstream HEAD 60 \
    > docs/benchmarks/decision-model.json
```

The absolute numbers depend on the accelerator and on the state of the model
in the cache. The distribution and the shape of the latency are the parts that
matter.

# Benchmark of the synchronisation of a fork

The second measurement is the work the dashboard does before the model is
asked anything: fetching the upstream branch of a fork, listing the commits the
fork is missing, and storing that list. The numbers come from Criterion over
the benchmark in
[`crates/fsrs-core/benches/sync.rs`](../../crates/fsrs-core/benches/sync.rs).

```bash
cargo bench -p fsrs-core
```

## The fixture

An upstream of 1500 real commits, cloned in full, then reset 200 commits back
— the backlog a user comes back to after a while. The commits each change one
small file, so every diff is real.

## The numbers

The numbers were measured on the same machine and with the same compiler,
before and after the changes of the sixth wave. The "before" column is the
code of the fifth wave, measured again so that the two columns can be
compared.

| Operation | Path | Before | After |
|-----------|------|--------|-------|
| Listing 200 pending commits | every diff is computed | 32.53 ms | 23.60 ms |
| Listing 200 pending commits | statistics known, no diff computed | 3.03 ms | 2.66 ms |
| Listing 200 pending commits | the first listing, nothing known | 36.74 ms | 24.42 ms |
| Listing 200 pending commits | metadata only, no diff at all | 2.87 ms | 2.64 ms |
| Statistics of one commit | two trees compared | 146.40 µs | 107.74 µs |
| Metadata of one commit | the commit object read | 283.90 ns | 281.67 ns |
| Storing 500 rows | the previous path: delete all, insert all | 1.00 ms | 1.00 ms |
| Storing 500 rows | every row written again | 378.65 µs | 222.20 µs |
| Storing 500 rows | the list did not move | 157.76 µs | 164.82 µs |
| Synchronisation end to end | cold, statistics unknown | 37.07 ms | 28.92 ms |
| Synchronisation end to end | warm, statistics known | 4.93 ms | 4.90 ms |
| Synchronisation end to end | the fetch alone, nothing is listed | — | 625.11 µs |
| Reading the statistics of 200 commits from the database | the path the interface used before | 125.46 µs | — |

The times are the medians of ten runs of Criterion. The rows of the store are
the ones of a database on the disk, and the store of the benchmark holds five
hundred rows of commits.

The listing of the pending commits is **27% faster**, and the statistics of
one commit are **26% faster**: the lines of a diff are counted in one walk of
the diff now, instead of two — see
[ADR-16](../adr/ADR-16-count-the-lines-of-a-commit-in-one-pass.md).

The row of the store that did not move costs the same as before, and it does
not write a single page now: the store writes the difference between the list
it has and the list it receives — see
[ADR-17](../adr/ADR-17-write-only-the-rows-that-really-changed.md). The row
that changed is 40% faster, because one row is written instead of five
hundred.

The warm synchronisation is **5.9x faster** than the cold one and **6.6x
faster** than the previous path, because the only thing that is computed is
what changed. It does not read the database any more: the worker keeps the
statistics of the commits in memory — see
[ADR-18](../adr/ADR-18-keep-the-statistics-of-the-commits-in-memory.md).

The statistics of one commit are **382x** more expensive than its metadata:
108 µs against 282 ns. The diff of the commit is the price of the listing,
and the metadata — the author, the message, the time — is free. The split of
the two halves is what makes the two numbers above possible: the listing with
the metadata only and no diff at all is as fast as the listing with the
statistics that are already known.

The fetch alone — the opening of the repository and the fetch of the upstream
— is 625 µs of the warm synchronisation, and the rest is the listing of the
commits that are pending.

## Why a commit is not measured twice

The fork is synchronised every time the dashboard opens. Between two
synchronisations of a fork that did not move, nothing about its pending commits
changes — a commit is immutable — so the diff statistics of the first listing
are kept in the repository itself, and reused on every later one. See
[ADR-09](../adr/ADR-09-commits-are-counted-once.md).

## The options of git2

The listing of the missing commits of a fork is the work the user waits for
when the dashboard opens. Two options of git2 stand in the way of it: the
cache of the parsed objects, which is useless when every object is read once,
and the verification of the hash of every object, which belongs to a check of
the repository and not to a working copy that was fetched over https.

The measurement was made on a real fork — the `fd` project, 1450 commits
behind its upstream — with the probe of the repository:

```bash
cargo run -p fsrs-core --example revwalk-probe -- <fork> <branch>
```

| Options | The whole listing | Of which the diff |
|---------|-------------------|------------------|
| Both on (the default) | 2500 ms | 2467 ms |
| The verification off | 2320 ms | 2261 ms |
| The cache off | 2450 ms | 2368 ms |
| Both off | **1270 ms** | 1226 ms |

The numbers are the medians of five runs. The walk of the graph of the commits
is not the expensive part — it takes 30 ms of the 2500 ms. The comparison of
the trees of every commit takes the other 2467 ms, and the two options halve
the time of the listing because they make the diff cheaper.

See [ADR-12](../adr/ADR-12-read-the-objects-as-fast-as-git-can.md).

# Benchmark of the slots of the decision server

The ratings of the backlog could be evaluated in parallel, but the server of
the model serves one request at a time per slot. The server of this machine
reports one slot:

```json
{"total_slots": 1, "default_generation_settings": {"n_ctx": 4096}}
```

The probe of the client measures the two ways of asking the same questions:

```bash
cargo run -p fsrs-ai --example slot-probe -- http://127.0.0.1:8089
```

| Requests | One after another | In parallel |
|----------|-------------------|-------------|
| 1 | 75 ms | 78 ms |
| 2 | 155 ms | 190 ms |
| 4 | 342 ms | 394 ms |
| 8 | 743 ms | 797 ms |

The parallel run is slower at every size: the requests wait in the queue of
the server, and the threads only add the cost of the context switch. The
numbers are the medians of five runs.

The dashboard therefore asks the server how many slots it has and never keeps
more ratings in flight than the server can serve. See
[ADR-13](../adr/ADR-13-rate-the-commits-one-at-a-time.md).

# Benchmark of the calls to the decision server

The third measurement is the traffic between the dashboard and the local
model. Every call is a round trip over the loopback, and the local model
answers in seconds — so a call that is not needed is a call the user waits
for.

The numbers come from the live server, with the counter of the client:

```bash
cargo test -p fsrs-ai --test live_server
```

## The calls of one rating

| Question | The previous path | The current path |
|----------|-------------------|------------------|
| The criticality of a commit | 1 `/completion` + 1 `/props` + the count of the tokens | 1 `/completion` |
| The complexity of the same commit | 1 `/completion` + 1 `/props` + the count of the tokens | 1 `/completion` |
| The whole rating | 6 calls | 4 calls |

The client is shared by every rating of the dashboard, so the size of the
window is asked once per process and not once per commit. The fitted context
is remembered too, so the second question of a commit — the complexity after
the criticality — reads the same context without asking the server to count
the tokens again.

## The series of commits

| Measurement | The previous path | The current path |
|-------------|-------------------|------------------|
| Three commits, small diffs | 12 calls, 12.4 s | 10 calls, 9.4 s |
| The window of the server | asked once per commit | asked once per process |

The series is the case that matters: the dashboard rates the backlog of a
fork commit after commit, and every commit used to pay for a handshake and a
fresh question about the window of the server.

## The fitting of a prompt

The fitting is what asks the server how many tokens a prompt holds. The
binary search asked the server once per step; the estimate asks it once for
the whole context and a few times to correct the cut. The numbers come from
Criterion over the benchmark in
[`crates/fsrs-ai/benches/context.rs`](../../crates/fsrs-ai/benches/context.rs),
with a window of 1024 tokens — a context of six thousand characters has to be
cut, which is the case where the two algorithms differ:

```bash
cargo bench -p fsrs-ai --bench context
```

| Algorithm | Calls to the server | Time |
|-----------|---------------------|------|
| The binary search | 14 | 48.5 µs |
| The estimate and the correction | 10 | 36.8 µs |

The times are of the algorithm alone, with a local tokenizer: the calls
themselves are measured against the live server, and they are the part that
costs seconds.
