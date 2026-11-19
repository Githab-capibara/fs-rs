#!/usr/bin/env bash
# Measure the time the dashboard needs to draw a frame.
#
# The window is drawn again and again while the pipeline runs, and every one
# of those frames costs the processor. The program writes the times of the
# frames to its log, and this script reads them out.
#
# The fork is a real one — the fork of the `fd` project is 1450 commits
# behind its upstream, so the window has 1450 rows to draw.
#
# ```bash
# scripts/measure-frames.sh target/release/fsrs /tmp/opencode/fd-fork 5
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

# The frames follow each other only when the program is told to draw them:
# otherwise it draws a frame when something changes, and the measurement would
# measure the frames that the pipeline happened to cause.
export FSRS_CONTINUOUS_REPAINT=1

echo "measuring the frames of $binary over $runs runs"

for run in $(seq 1 "$runs"); do
    rm -f "${work}/fsrs.db"
    log="${work}/run-${run}.log"
    "$binary" --config "${work}/config.toml" --data "${work}" > "${log}" 2>&1 &
    pid=$!
    line=""
    # The window has to draw a few hundred frames before the numbers mean
    # something: the first frames are the ones that pay for the start.
    for _ in $(seq 1 1200); do
        if ! kill -0 "$pid" 2> /dev/null; then
            break
        fi
        line="$(sed 's/\x1b\[[0-9;]*m//g' "${log}" |
            grep -o 'the frames of the window.*' | tail -1 || true)"
        if [ -n "$line" ]; then
            break
        fi
        sleep 0.05
    done
    kill "$pid" 2> /dev/null || true
    wait "$pid" 2> /dev/null || true
    if [ -z "$line" ]; then
        echo "the run $run did not report the frames" >&2
        continue
    fi
    echo "run $run: $line"
done
