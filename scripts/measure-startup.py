#!/usr/bin/env python3
"""Compare the startup time of several binaries, one run after another.

The startup of a program is measured by running it and waiting for it to
finish. One run says little: the machine does something else in the
background, the cache is cold or warm, the scheduler is busy or idle. The
runs are therefore interleaved — the first binary, the second, the first
again — and the median of every binary is reported at the end.

```bash
scripts/measure-startup.py target/release/fsrs target/pgo/release/fsrs 30
```
"""

import statistics
import subprocess
import sys
import time


def measure(binary, runs):
    """Return the times of the runs of one binary, in microseconds."""
    times = []
    for _ in range(runs):
        started = time.perf_counter_ns()
        subprocess.run([binary, "--help"], stdout=subprocess.DEVNULL, check=True)
        finished = time.perf_counter_ns()
        times.append((finished - started) // 1000)
    return times


def main(argv):
    if len(argv) < 2:
        print("usage: measure-startup.py <binary> [<binary>...] [--runs N]")
        return 2

    runs = 30
    binaries = []
    arguments = argv[1:]
    while arguments:
        argument = arguments.pop(0)
        if argument == "--runs":
            runs = int(arguments.pop(0))
        else:
            binaries.append(argument)

    if not binaries:
        print("there is no binary to measure")
        return 2

    results = {binary: [] for binary in binaries}
    # The runs are interleaved: the machine is in the same state for every
    # binary of a round.
    for _ in range(runs):
        for binary in binaries:
            results[binary].extend(measure(binary, 1))

    width = max(len(binary) for binary in binaries)
    print(f"{'binary'.ljust(width)}  runs  min_us  median_us  max_us")
    for binary in binaries:
        times = sorted(results[binary])
        print(
            f"{binary.ljust(width)}  {len(times):5d}  {times[0]:6d}  "
            f"{int(statistics.median(times)):9d}  {times[-1]:6d}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
