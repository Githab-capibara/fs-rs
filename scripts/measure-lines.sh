#!/usr/bin/env bash
# Measure how much code every crate of the workspace holds.
#
# `cargo llvm-lines` counts the lines of the intermediate representation of
# LLVM that every function of a crate produced. The number says how big a
# function is, and the biggest functions are the ones that are worth looking
# at when the program is slow.
#
# The count is of the whole program, not of the source: a generic function of
# a dependency is counted once for every type that it was made for, and the
# names of such a function are long. The script cuts the names, so that the
# table can be read.
#
# ```bash
# scripts/measure-lines.sh
# scripts/measure-lines.sh 15
# ```

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

top="${1:-10}"
crates=(fsrs-core fsrs-ai fsrs-acp fsrs-app)

if ! cargo llvm-lines --version > /dev/null 2>&1; then
    echo "cargo llvm-lines is not installed: cargo install cargo-llvm-lines" >&2
    exit 1
fi

for crate in "${crates[@]}"; do
    echo "### ${crate}"
    # The names of the generic functions are cut to their last part: the
    # monomorphisation of the protocol of the agents produces names of many
    # hundreds of characters.
    cargo llvm-lines -p "${crate}" --lib 2> /dev/null |
        awk -v top="${top}" '
            /Lines/ { next }
            /-----/ { next }
            /^\s*$/ { next }
            /TOTAL/ { printf "%8s %8s  %s\n", $1, $2, "(TOTAL)"; next }
            {
                lines = $1
                copies = $4
                # The name starts after the two numbers and their share.
                name = $0
                sub(/^.*%\)[[:space:]]+[0-9]+ \([0-9.]+%,[[:space:]]+[0-9.]+%\)[[:space:]]+/, "", name)
                if (name == "") { next }
                count += 1
                if (count > top) { next }
                # The last part of the name of a generic function is enough.
                gsub(/::\{closure#[0-9]+\}/, "", name)
                if (length(name) > 110) { name = "…" substr(name, length(name) - 109) }
                printf "%8s %8s  %s\n", lines, copies, name
            }
        '
done
