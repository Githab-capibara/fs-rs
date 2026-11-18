#!/usr/bin/env bash
# Measure the time the dashboard needs to list the missing commits of a fork.
#
# The dashboard reads the objects of the fork when it starts: the listing of
# the commits that the fork does not have yet is the operation the user waits
# for. The binary reports the time of the listing in its log, and this script
# reads that number.
#
# ```bash
# scripts/measure-sync.sh target/release/fsrs /tmp/opencode/fd-fork master 5
# ```

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

binary="${1:-target/release/fsrs}"
repository="${2:-/tmp/opencode/fd-fork}"
branch="${3:-master}"
runs="${4:-5}"

if [ ! -x "$binary" ]; then
    echo "there is no binary at $binary" >&2
    exit 1
fi

work="$(mktemp -d /tmp/opencode/sync-measure.XXXXXX)"
trap 'rm -rf "$work"' EXIT

cat > "${work}/config.toml" <<EOF
[[repos]]
name = "fd"
path = "${repository}"
upstream_url = ""
upstream_web = ""
main_branch = "${branch}"
EOF

echo "measuring the listing of the commits of $binary over $runs runs"

times=()
for run in $(seq 1 "$runs"); do
    rm -f "${work}/fsrs.db"
    log="${work}/run-${run}.log"
    # The window of the dashboard is opened and stays open, so the process is
    # stopped as soon as the listing is over.
    "$binary" --config "${work}/config.toml" --data "${work}" > "${log}" 2>&1 &
    pid=$!
    elapsed=""
    for _ in $(seq 1 600); do
        if ! kill -0 "$pid" 2> /dev/null; then
            break
        fi
        # The log of the dashboard is coloured, and the colours put escape
        # sequences between the name of the field and its value, so they are
        # taken out before the number is read.
        line="$(sed 's/\x1b\[[0-9;]*m//g' "${log}" | grep -o 'sync finished.*' | head -1 || true)"
        if [ -n "$line" ]; then
            elapsed="$(echo "$line" | grep -o 'elapsed_ms=[0-9]*' | cut -d= -f2)"
            break
        fi
        sleep 0.1
    done
    kill "$pid" 2> /dev/null || true
    wait "$pid" 2> /dev/null || true
    if [ -z "$elapsed" ]; then
        echo "the run $run did not report the listing" >&2
        continue
    fi
    times+=("$elapsed")
    echo "run $run: ${elapsed} ms"
done

if [ "${#times[@]}" -eq 0 ]; then
    echo "no run reported the listing" >&2
    exit 1
fi

printf '%s\n' "${times[@]}" | sort -n | awk '
    { values[NR] = $1 }
    END {
        middle = int((NR + 1) / 2)
        printf "runs=%d\n", NR
        printf "min_ms=%d\n", values[1]
        printf "median_ms=%d\n", values[middle]
        printf "max_ms=%d\n", values[NR]
    }
'
