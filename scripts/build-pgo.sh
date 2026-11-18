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
#   scripts/build-pgo.sh --bolt             # build with the profile and BOLT
#
# The training command is the work the program really does. The default one
# rates the commits of a real repository with the local decision model:
#
#   FSRS_PGO_REPOSITORY=/path/to/repo FSRS_PGO_REVISION=HEAD \
#       scripts/build-pgo.sh
#
# The result is in `target/pgo/`, the final binary is
# `target/pgo/release/fsrs`.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Where the instrumented binary and the profiles live.
#
# The path is absolute: the compiler of a crate of the workspace runs in the
# directory of that crate, and a relative path would send the profiles of
# every crate to a different place.
profile_dir="${FSRS_PGO_DIR:-target/pgo}"
case "$profile_dir" in
    /*) ;;
    *) profile_dir="${root}/${profile_dir}" ;;
esac

# The number of runs of the training. Every run adds a little more of the
# program to the profile.
runs="${FSRS_PGO_RUNS:-3}"

# The command that trains the profile. It has to touch the parts of the
# program that matter: the reading of the repositories, the listing of the
# commits and the drawing of the window.
train_command="${FSRS_PGO_COMMAND:-}"

# The default training command. It rates the commits of a real repository
# with the local decision model, so the profile is of the work the program
# really does: reading the git objects, building the diff of a commit,
# fitting the prompt into the window of the model and sending it.
#
# The repository and the revision can be changed through the environment.
# The listing of the missing commits of a fork is the second part of the
# work, and it is trained too when a repository is named:
#
#   FSRS_PGO_GIT_REPOSITORY=/path/to/fork FSRS_PGO_GIT_BRANCH=master
#
default_train_command() {
    local repository="${FSRS_PGO_REPOSITORY:-}"
    local revision="${FSRS_PGO_REVISION:-HEAD}"
    local limit="${FSRS_PGO_LIMIT:-6}"
    local git_repository="${FSRS_PGO_GIT_REPOSITORY:-}"
    local git_branch="${FSRS_PGO_GIT_BRANCH:-master}"
    local steps=()
    if [ -z "$repository" ]; then
        # Without a repository the startup is trained: the reading of the
        # configuration, the opening of the store and the drawing of the
        # first frame.
        steps+=("${profile_dir}/train/release/fsrs --help")
    else
        steps+=("${profile_dir}/train/release/examples/benchmark ${repository} ${revision} ${limit}")
    fi
    if [ -n "$git_repository" ]; then
        steps+=("${profile_dir}/train/release/examples/revwalk-probe ${git_repository} ${git_branch}")
    fi
    local command=""
    for step in "${steps[@]}"; do
        if [ -n "$command" ]; then
            command="${command} && "
        fi
        command="${command}${step}"
    done
    echo "$command"
}

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
    train_command="$(default_train_command)"
fi

mkdir -p "$profile_dir"

echo "==> building the instrumented binary"
CARGO_PROFILE_RELEASE_DEBUG=1 \
RUSTFLAGS="-Cprofile-generate=${profile_dir}/default.profraw" \
    cargo build --release --target-dir "${profile_dir}/train"
# The training command is run from the instrumented directory, so the tools
# it needs are built there too: the example that rates the commits of a
# repository and the probe that lists the missing commits of a fork are the
# trainers of the default command.
if [ -z "${FSRS_PGO_COMMAND:-}" ]; then
    CARGO_PROFILE_RELEASE_DEBUG=1 \
    RUSTFLAGS="-Cprofile-generate=${profile_dir}/default.profraw" \
        cargo build --release --target-dir "${profile_dir}/train" \
            --workspace --examples
fi

echo "==> gathering the profile"
for run in $(seq 1 "$runs"); do
    echo "--- run $run of $runs"
    LLVM_PROFILE_FILE="${profile_dir}/default-%p-%m.profraw" \
        bash -c "$train_command"
done

# A run that failed before it reached the program leaves no profile behind.
# The build with a profile that does not exist is a build without a profile,
# and the compiler says so in a way that is easy to miss.
if ! compgen -G "${profile_dir}/*.profraw" > /dev/null; then
    echo "the training runs left no profile behind" >&2
    echo "the training command has to run the instrumented program" >&2
    echo "training command: $train_command" >&2
    exit 1
fi

echo "==> merging the profiles"
"$profdata" merge -o "${profile_dir}/default.profdata" "${profile_dir}"/*.profraw

if [ "${1:-}" = "--train" ]; then
    echo "the profile is in ${profile_dir}/default.profdata"
    exit 0
fi

echo "==> building the final binary"
# BOLT lays the code out again, and it needs the relocations of the code to
# do it: the linker has to leave them in the binary. The linker refuses to
# keep the relocations of a stripped binary, so the symbols stay until BOLT
# is done with them.
link_flags=""
if [ "${1:-}" = "--bolt" ]; then
    # The linker refuses to keep the relocations of a stripped binary, so the
    # symbols stay until BOLT is done with them.
    link_flags="-C link-args=-Wl,--emit-relocs"
    CARGO_PROFILE_RELEASE_STRIP=none \
    RUSTFLAGS="-Cprofile-use=${profile_dir}/default.profdata ${link_flags}" \
        cargo build --release --target-dir "${profile_dir}"
else
    RUSTFLAGS="-Cprofile-use=${profile_dir}/default.profdata" \
        cargo build --release --target-dir "${profile_dir}"
fi

binary="${profile_dir}/release/fsrs"

# BOLT lays the code of the binary out again: it moves the functions that are
# really called next to each other, merges the ones that do the same thing and
# splits the cold code away from the hot one. The tool has to be the one of
# the same LLVM as the compiler — a mismatch makes the tool refuse the
# binary.
if [ "${1:-}" = "--bolt" ]; then
    bolt="${FSRS_LLVM_BOLT:-}"
    if [ -z "$bolt" ]; then
        if command -v llvm-bolt > /dev/null 2>&1; then
            bolt="$(command -v llvm-bolt)"
        else
            echo "the build needs llvm-bolt" >&2
            echo "install the llvm-bolt package, or set FSRS_LLVM_BOLT to the path of the tool" >&2
            exit 1
        fi
    fi
    if [ ! -x "$bolt" ]; then
        echo "the build needs llvm-bolt of the same LLVM as the compiler" >&2
        echo "set FSRS_LLVM_BOLT to the path of the tool" >&2
        exit 1
    fi

    # The profile of the run in the format of BOLT. Without it the tool still
    # works, but it only merges the functions that do the same thing — the
    # layout of the hot paths needs the profile.
    bolt_data=""
    if [ -n "${FSRS_PGO_BOLT_DATA:-}" ]; then
        if [ ! -f "${FSRS_PGO_BOLT_DATA}" ]; then
            echo "the profile of BOLT is not at ${FSRS_PGO_BOLT_DATA}" >&2
            exit 1
        fi
        bolt_data="${FSRS_PGO_BOLT_DATA}"
    fi

    echo "==> applying BOLT"
    if [ -n "$bolt_data" ]; then
        "$bolt" "$binary" -o "${binary}.bolt" \
            --icf=1 \
            --relocs \
            --update-debug-sections \
            --skip-funcs=__rust_probestack \
            -data="$bolt_data"
    else
        "$bolt" "$binary" -o "${binary}.bolt" \
            --icf=1 \
            --relocs \
            --update-debug-sections \
            --skip-funcs=__rust_probestack
    fi
    mv "${binary}.bolt" "$binary"

    # The symbols were kept for BOLT — the linker refuses to keep the
    # relocations of a stripped binary — and they are taken out now.
    if command -v llvm-strip > /dev/null 2>&1; then
        llvm-strip --strip-all "$binary"
    elif command -v strip > /dev/null 2>&1; then
        strip --strip-all "$binary"
    fi
fi

echo "the binary is in $binary"
