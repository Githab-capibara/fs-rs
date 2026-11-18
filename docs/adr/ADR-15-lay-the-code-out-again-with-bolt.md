# 15. Lay the code out again with BOLT

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** @Githab-capibara
- **Related:** [ADR-11](ADR-11-build-for-the-machine.md),
  [`scripts/build-pgo.sh`](../../scripts/build-pgo.sh),
  [`scripts/measure-startup.py`](../../scripts/measure-startup.py),
  [`scripts/measure-sync.sh`](../../scripts/measure-sync.sh)

## Context

The fortieth point of the checklist of the performance work asks for the
binary to be laid out again by BOLT — the tool of LLVM that moves the hot
functions next to each other, merges the ones that do the same thing and
splits the cold code away from the hot one.

ADR-11 rejected the idea, because the tool was not in the toolchain of rust.
The tool is not there now either: rustup does not ship it, and the Arch
package of it carries a different version of LLVM than the compiler of rust
uses. The tool was still tried, because the checklist asks for it and because
a decision without a measurement is a guess.

The versions do not stop the tool. The compiler of this machine is rustc
1.98.1, whose LLVM is 22.1.8; the BOLT of the Arch archive is 23.1.5. The
tool was applied to the binary of the dashboard and accepted it: it read the
sections, the symbols and the relocations of the binary, and it wrote a new
one that runs.

The profile of the run is what makes the tool work. BOLT reads a profile in
its own format, and `perf2bolt` makes one out of the data of `perf`. The
profile of this project was recorded from the dashboard itself: the window was
opened against a real fork — the `fd` project, 1450 commits behind its
upstream — and the program was left to read the objects and to rate the
commits while `perf` watched it.

```bash
perf record -e branches:u -j any,u -c 20000 -o perf.data target/pgo/release/fsrs
perf2bolt -p perf.data target/pgo/release/fsrs -o data.fdata
llvm-bolt target/pgo/release/fsrs -o fsrs --relocs -data=data.fdata
```

The profile of that run holds 143600 samples and 4594907 branch entries. It
covers 556 of the 10768 functions of the binary — the ones the run really
reached.

## Decision

The build script applies BOLT when it is asked to, and the script of the
profile is kept in the repository so the decision can be checked again:

```bash
FSRS_PGO_REPOSITORY=/path/to/repo FSRS_PGO_GIT_REPOSITORY=/path/to/fork \
    scripts/build-pgo.sh --bolt
```

BOLT needs the relocations of the code to lay it out again, and the linker
does not keep the relocations of a stripped binary. The binary is therefore
linked without stripping when BOLT is asked for, and the symbols are taken
out afterwards.

The tool is optional in the build. The ordinary build does not need it, and
the build with a profile does not need it either — the profile of the
compiler already lays the hot paths out first.

## Consequences

- **Easier:** the build has one more step that can be asked for, and the
  measurement of it is in the repository: the script
  [`scripts/measure-startup.py`](../../scripts/measure-startup.py) compares
  the binaries one run after another, and
  [`scripts/measure-sync.sh`](../../scripts/measure-sync.sh) measures the
  listing of the missing commits of a real fork.
- **Harder:** the build with BOLT needs three tools that are not in the
  toolchain of rust: `llvm-profdata`, `llvm-bolt` and `perf`. The first is
  in the toolchain, the other two have to be installed. The version of BOLT
  has to be able to read the binary of the compiler — the one of the Arch
  archive does, the one of another distribution may not.
- **Given up:** nothing is given up by the decision, because the tool is
  optional. What the measurement gives up is the hope of a faster program:
  the numbers below say that the layout of the code is not where the time of
  the dashboard goes.
- **Migration:** none. The change is inside the build.

## The numbers

The comparison is of three binaries of the same program, built from the same
source: the ordinary release build, the build with the profile of the
compiler, and the build with the profile of the compiler and BOLT on top of
it. The startup is the median of forty interleaved runs of `--help`:

| Binary | The start of the program |
|--------|--------------------------|
| The ordinary build | 3364 µs |
| The build with the profile | 3312 µs |
| The build with the profile and BOLT | 3362 µs |

The listing of the missing commits of the fork — the operation that takes
1.2 seconds and is the only real wait of the program — is the median of five
runs:

| Binary | The listing of the commits |
|--------|---------------------------|
| The ordinary build | 1245 ms |
| The build with the profile | 1254 ms |
| The build with the profile and BOLT | 1223 ms |

The differences are inside the noise of the machine.

The reason is visible in the profile itself: 5.2% of the functions of the
binary have a profile. The dashboard spends its time in the code of `libgit2`
— the comparison of the trees of every commit — and in the model that answers
the questions, and both are outside the code that BOLT can move around. The
window of the program draws its frames in `egui` and in the renderer of the
graphics card, and the frames are not the wait of the user.

BOLT does what it promises to the functions it knows: it shortened 32552
instructions, removed 776 empty blocks, folded 9 functions into the ones they
do the same thing as, and moved the hot blocks of 481 functions to the front
of their function. The program does not spend its time there.

## Alternatives considered

- **Option A:** make BOLT a required part of the build. Rejected: the tool is
  not in the toolchain, three tools have to be installed for it, and the
  measurement says that the time of the program is not in the code it moves.
- **Option B:** build the binary without the position-independent code, so
  that BOLT rewrites it in place instead of keeping the original text next to
  the new one. Tried and measured: the flag has to be given to the link of
  the binary alone — `cargo rustc --release -p fsrs-app --bin fsrs -- -C
  link-args=-no-pie` — because the whole workspace cannot be linked that way,
  the shared objects of the build scripts of the dependencies are not
  position-independent. The binary that comes out of it is 20.8 MiB instead of
  21.8 MiB, and BOLT rewrites 275 of the 16690 functions of it in place. The
  start of the program is 3365 µs without the tool and 3362 µs with it, and
  the listing of the commits is 1231 ms and 1215 ms — the differences are
  inside the noise. The binary would also lose the randomisation of its
  address space, which is not worth a gain that is not there.
- **Option C:** use the profile of the compiler only. Accepted as the way the
  program is built: it needs one tool of the toolchain, it makes the binary
  smaller, and it is the profile of the same workload.
