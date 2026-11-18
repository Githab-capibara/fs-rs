#!/usr/bin/env bash
# Record the profile that BOLT needs.
#
# BOLT moves the functions that are really called next to each other, and it
# learns which ones they are from a profile of a real run. The profile of
# `perf` is turned into the profile of BOLT by `perf2bolt`, which is the tool
# of the same package as `llvm-bolt`.
#
# The dashboard is opened against a real repository, so the profile is of the
# work the program really does: reading the objects of the fork, listing the
# commits that it is missing and rating them.
#
# ```bash
# FSRS_BOLT_REPOSITORY=/path/to/repo FSRS_BOLT_GIT_REPOSITORY=/path/to/fork \
#     scripts/record-bolt-profile.sh target/pgo/release/fsrs
# ```
#
# The result is `target/pgo/bolt.fdata`, and it is given to the build:
#
# ```bash
# FSRS_PGO_BOLT_DATA=target/pgo/bolt.fdata scripts/build-pgo.sh --bolt
# ```

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

binary="${1:-target/pgo/release/fsrs}"

if [ ! -x "$binary" ]; then
    echo "there is no binary at $binary" >&2
    exit 1
fi

# The tools. The path of the perf of the machine can be given, because it is
# not always in the path of the shell.
perf="${FSRS_PERF:-}"
if [ -z "$perf" ]; then
    if command -v perf > /dev/null 2>&1; then
        perf="$(command -v perf)"
    else
        echo "the recording needs perf" >&2
        echo "set FSRS_PERF to the path of the tool" >&2
        exit 1
    fi
fi

perf2bolt="${FSRS_PERF2BOLT:-}"
if [ -z "$perf2bolt" ]; then
    if command -v perf2bolt > /dev/null 2>&1; then
        perf2bolt="$(command -v perf2bolt)"
    else
        echo "the recording needs perf2bolt" >&2
        echo "set FSRS_PERF2BOLT to the path of the tool" >&2
        exit 1
    fi
fi

# The repository the dashboard reads, and the fork it is compared with.
repository="${FSRS_BOLT_REPOSITORY:-}"
git_repository="${FSRS_BOLT_GIT_REPOSITORY:-}"
branch="${FSRS_BOLT_GIT_BRANCH:-master}"
duration="${FSRS_BOLT_DURATION:-120}"

if [ -z "$repository" ] && [ -z "$git_repository" ]; then
    echo "the recording needs a repository to work on" >&2
    echo "set FSRS_BOLT_REPOSITORY and FSRS_BOLT_GIT_REPOSITORY" >&2
    exit 1
fi

work="$(mktemp -d /tmp/opencode/bolt-profile.XXXXXX)"
trap 'rm -rf "$work"' EXIT

cat > "${work}/config.toml" <<EOF
[[repos]]
name = "bolt"
path = "${git_repository:-${repository}}"
upstream_url = ""
upstream_web = ""
main_branch = "${branch}"
EOF

echo "==> recording the profile of $binary for ${duration} seconds"

# The branches are sampled with the stack of the branches, because the
# profile of BOLT is made of the paths the program really took.
timeout "$duration" "$perf" record \
    -e branches:u \
    -j any,u \
    -c 20000 \
    -o "${work}/perf.data" \
    "$binary" \
    --config "${work}/config.toml" \
    --data "${work}" || true

if [ ! -s "${work}/perf.data" ]; then
    echo "the recording left no profile behind" >&2
    echo "the machine has to allow the sampling of the branches" >&2
    exit 1
fi

echo "==> turning the profile into the format of BOLT"
# The tool of perf has to be in the path: perf2bolt calls it to read the
# samples.
PATH="$(dirname "$perf"):$PATH" \
    "$perf2bolt" -p "${work}/perf.data" "$binary" -o "${work}/bolt.fdata"

output="${FSRS_BOLT_OUTPUT:-target/pgo/bolt.fdata}"
mkdir -p "$(dirname "$output")"
cp "${work}/bolt.fdata" "$output"

echo "the profile of BOLT is in $output"
echo "use it with: FSRS_PGO_BOLT_DATA=$output scripts/build-pgo.sh --bolt"
