#!/usr/bin/env bash
# Build the dashboard with a profile-guided optimisation.
#
# The profile is gathered by running the program with the instrumented
# binary, so the compiler knows which code the program really uses. The
# second build then lays the hot paths out first.
#
# The build needs three tools:
#
#   * `llvm-profdata` — it merges the profiles of the runs. It has to be the
#     one of the same toolchain as the compiler: the raw profile format
#     changes between the versions, and a profile of one version cannot be
#     read by another. The toolchain of rustup ships the right one:
#
#         rustup component add llvm-tools-preview
#
#   * a training command — the program has to be run, so the profile is of
#     a real workload and not of an empty startup,
#   * `cargo`.
#
# The steps are:
#
#   1. build the instrumented binary,
#   2. run it once, so the profile is filled,
#   3. merge the profiles,
#   4. build the final binary with the profile.
#
# Usage:
#
#   FSRS_PGO_COMMAND='./target/pgo/train/release/fsrs --help' \
#       scripts/build-pgo.sh                 # build with the profile
#   FSRS_PGO_COMMAND='...' scripts/build-pgo.sh --train   # only the profile
#
# The result is in `target/pgo/`, the final binary is
# `target/pgo/release/fsrs`.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Where the instrumented binary and the profiles live.
profile_dir="${FSRS_PGO_DIR:-target/pgo}"

# The number of runs of the training. Every run adds a little more of the
# program to the profile.
runs="${FSRS_PGO_RUNS:-3}"

# The command that trains the profile. It has to touch the parts of the
# program that matter: the reading of the repositories, the listing of the
# commits and the drawing of the window.
train_command="${FSRS_PGO_COMMAND:-}"

require() {
    if ! command -v "$1" > /dev/null 2>&1; then
        echo "the build needs $1" >&2
        exit 1
    fi
}

require cargo

# The tool of the toolchain: the raw profile format has to match the one of
# the compiler, and the versions drift apart.
profdata="${FSRS_LLVM_PROFDATA:-}"
if [ -z "$profdata" ]; then
    sysroot="$(rustc --print sysroot)"
    profdata="${sysroot}/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata"
fi
if [ ! -x "$profdata" ]; then
    echo "the build needs llvm-profdata of the same toolchain as the compiler" >&2
    echo "run: rustup component add llvm-tools-preview" >&2
    echo "or set FSRS_LLVM_PROFDATA to the path of the tool" >&2
    exit 1
fi

if [ -z "$train_command" ]; then
    echo "no training command was given" >&2
    echo "set FSRS_PGO_COMMAND to the command that exercises the program," >&2
    echo "for example:" >&2
    echo "  FSRS_PGO_COMMAND='${profile_dir}/train/release/fsrs --help'" >&2
    exit 2
fi

mkdir -p "$profile_dir"

echo "==> building the instrumented binary"
CARGO_PROFILE_RELEASE_DEBUG=1 \
RUSTFLAGS="-Cprofile-generate=${profile_dir}/default.profraw" \
    cargo build --release --target-dir "${profile_dir}/train"

echo "==> gathering the profile"
for run in $(seq 1 "$runs"); do
    echo "--- run $run of $runs"
    LLVM_PROFILE_FILE="${profile_dir}/default-%p-%m.profraw" \
        bash -c "$train_command"
done

echo "==> merging the profiles"
"$profdata" merge -o "${profile_dir}/default.profdata" "${profile_dir}"/*.profraw

if [ "${1:-}" = "--train" ]; then
    echo "the profile is in ${profile_dir}/default.profdata"
    exit 0
fi

echo "==> building the final binary"
RUSTFLAGS="-Cprofile-use=${profile_dir}/default.profdata" \
    cargo build --release --target-dir "${profile_dir}"

echo "the binary is in ${profile_dir}/release/fsrs"
