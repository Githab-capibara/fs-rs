#!/usr/bin/env bash
# Measure the time the dashboard needs to draw its first frame.
#
# The first frame is what the user waits for after the click on the icon: the
# window is created, the surface is made and the first draw happens. The
# program reports the time in its log, and this script reads that number.
#
# ```bash
# scripts/measure-first-frame.sh target/release/fsrs /tmp/opencode/fd-fork 5
# ```

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

binary="${1:-target/release/fsrs}"
repository="${2:-/tmp/opencode/fd-fork}"
runs="${3:-5}"

if [ ! -x "$binary" ]; then
    echo "there is no binary at $binary" >&2
    exit 1
fi

work="$(mktemp -d /tmp/opencode/frame-measure.XXXXXX)"
trap 'rm -rf "$work"' EXIT

cat > "${work}/config.toml" <<EOF
[[repos]]
name = "fd"
path = "${repository}"
upstream_url = ""
upstream_web = ""
main_branch = "master"
EOF

echo "measuring the first frame of $binary over $runs runs"

times=()
for run in $(seq 1 "$runs"); do
    rm -f "${work}/fsrs.db"
    log="${work}/run-${run}.log"
    # The window of the dashboard is opened and stays open, so the process is
    # stopped as soon as the first frame is drawn.
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
        line="$(sed 's/\x1b\[[0-9;]*m//g' "${log}" |
            grep -o 'the first frame of the window was drawn.*' | head -1 || true)"
        if [ -n "$line" ]; then
            elapsed="$(echo "$line" | grep -o 'elapsed_ms=[0-9]*' | cut -d= -f2)"
            break
        fi
        sleep 0.05
    done
    kill "$pid" 2> /dev/null || true
    wait "$pid" 2> /dev/null || true
    if [ -z "$elapsed" ]; then
        echo "the run $run did not report the first frame" >&2
        continue
    fi
    times+=("$elapsed")
    echo "run $run: ${elapsed} ms"
done

if [ "${#times[@]}" -eq 0 ]; then
    echo "no run reported the first frame" >&2
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
