#!/usr/bin/env bash
# Profile a real run of the dashboard and print where the time goes.
#
# The profile is taken while the dashboard does the work the user waits for:
# it reads the objects of a real fork, lists the commits that the fork is
# missing and rates them with the local model. The report is the list of the
# functions the program spent its time in.
#
# ```bash
# FSRS_PERF=/path/to/perf scripts/profile.sh target/release/fsrs /tmp/opencode/fd-fork
# ```
#
# The machine has to allow the sampling of the events of the processor. The
# permission is read with:
#
# ```bash
# cat /proc/sys/kernel/perf_event_paranoid
# ```
#
# The value 2 allows the sampling of the events of the program itself, which
# is what this script needs. The sampling of the branches with their stack
# (`-j any,u`) needs the value 1 or lower, and it is the profile that BOLT
# reads — see `scripts/record-bolt-profile.sh`.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

binary="${1:-target/release/fsrs}"
repository="${2:-/tmp/opencode/fd-fork}"
branch="${3:-master}"
duration="${FSRS_PROFILE_DURATION:-60}"
limit="${FSRS_PROFILE_LIMIT:-40}"

if [ ! -x "$binary" ]; then
    echo "there is no binary at $binary" >&2
    exit 1
fi

perf="${FSRS_PERF:-}"
if [ -z "$perf" ]; then
    if command -v perf > /dev/null 2>&1; then
        perf="$(command -v perf)"
    else
        echo "the profiling needs perf" >&2
        echo "set FSRS_PERF to the path of the tool" >&2
        exit 1
    fi
fi

work="$(mktemp -d /tmp/opencode/profile.XXXXXX)"
trap 'rm -rf "$work"' EXIT

cat > "${work}/config.toml" <<EOF
[[repos]]
name = "profile"
path = "${repository}"
upstream_url = ""
upstream_web = ""
main_branch = "${branch}"
EOF

echo "==> profiling $binary against $repository for ${duration} seconds"

# The program keeps running until the time is over: it is stopped by the
# timeout, and the profile is written when it stops.
timeout "$duration" "$perf" record \
    -e cycles:u \
    -c 10000 \
    -o "${work}/perf.data" \
    "$binary" \
    --config "${work}/config.toml" \
    --data "${work}" > "${work}/program.log" 2>&1 || true

if [ ! -s "${work}/perf.data" ]; then
    echo "the recording left no profile behind" >&2
    echo "the machine has to allow the sampling of the events" >&2
    exit 1
fi

echo "==> the time of the program"

# The log of the dashboard says how long the listing of the commits took.
sed 's/\x1b\[[0-9;]*m//g' "${work}/program.log" |
    grep -o 'sync finished.*' | head -1 || true

echo
echo "==> the functions the program spent its time in"

"$perf" report \
    --stdio \
    --no-children \
    --percent-limit 0.5 \
    --sort symbol \
    -i "${work}/perf.data" |
    head -n "$((limit + 20))"
