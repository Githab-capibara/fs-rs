#!/usr/bin/env python3
"""Check that the documentation agrees with the code.

Every claim of the documentation that names a constant, a default or a number
is compared with the place that defines it. The script is a part of the
repository, so a change of the code that breaks the documentation is caught
before the documentation is read.

```bash
python3 scripts/verify-docs.py
```

The script needs nothing but Python. The checks that talk to the running
decision model are skipped when it does not answer.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

CHECKS = []


def check(question, ok, evidence):
    """Record one check with its evidence."""
    CHECKS.append((question, bool(ok), str(evidence)))


def read(relative):
    """Read a file of the repository."""
    return (ROOT / relative).read_text()


def runtime_props():
    """Ask the running decision model for its properties, if it answers."""
    try:
        answer = subprocess.run(
            ["curl", "-s", "-m", "5", "http://127.0.0.1:8089/props"],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return ""
    return answer


# --- The runtime of the decision model -------------------------------
UNIT = pathlib.Path.home() / ".config/systemd/user/foq-server.service"
if UNIT.exists():
    unit = UNIT.read_text()
    check(
        "The guide shows the arguments that the service really uses",
        "-c 4096" in unit and "-np 1" in unit and "--lora" in unit,
        "foq-server.service",
    )

props = runtime_props()
if props:
    window = re.search(r'"n_ctx":(\d+)', props)
    slots = re.search(r'"total_slots":(\d+)', props)
    check(
        "The window of the runtime is the one that the documentation names",
        bool(window) and window.group(1) == "4096" and slots.group(1) == "1",
        f"n_ctx={window.group(1) if window else '?'} slots={slots.group(1) if slots else '?'}",
    )
else:
    print("SKIP the running runtime does not answer")

# --- The defaults of the configuration -------------------------------
core = read("crates/fsrs-core/src/lib.rs")
check(
    "The configuration guide lists the real default of the address",
    'foq_url: "http://127.0.0.1:8089"' in core,
    "the default of foq_url",
)
check(
    "The timeout of 120 seconds is the real default",
    "foq_timeout_secs: 120" in core,
    "the default of foq_timeout_secs",
)
check(
    "The command of the agent is the real default",
    'acp_command: "opencode acp"' in core,
    "the default of acp_command",
)
check(
    "The command of the terminal is the real default",
    'terminal_command: "opencode mini"' in core,
    "the default of terminal_command",
)

# --- The decision model ----------------------------------------------
calibration = read("crates/fsrs-ai/src/calibration.rs")
check(
    "The temperatures of the profiles are the ones in the code",
    "2.6353" in calibration and "3.6381" in calibration,
    "BARE_TEMPERATURE and LORA_TEMPERATURE",
)
prompt = read("crates/fsrs-ai/src/prompt.rs")
check(
    "The patch of the context is cut at 6000 characters",
    "MAX_PATCH_CHARS: usize = 6000" in prompt,
    "MAX_PATCH_CHARS",
)
check(
    "The list of the files of the context is cut at 40 names",
    "MAX_LISTED_FILES: usize = 40" in prompt,
    "MAX_LISTED_FILES",
)

# --- The pipeline ----------------------------------------------------
prompts = read("crates/fsrs-acp/src/prompts.rs")
check(
    "The diff of a prompt is cut at 20000 characters",
    "MAX_PROMPT_DIFF_CHARS: usize = 20_000" in prompts,
    "MAX_PROMPT_DIFF_CHARS",
)
check(
    "The reviewer is asked for the linters of the documentation",
    all(
        linter in prompts
        for linter in (
            "cargo fmt --check",
            "cargo clippy",
            "cargo test",
            "shellcheck",
            "oxlint",
            "typos",
        )
    ),
    "the linters of the reviewer prompt",
)
pipeline = read("crates/fsrs-acp/src/pipeline.rs")
check(
    "The branches use the prefix that the code creates",
    'BRANCH_PREFIX: &str = "fsrs/implement"' in pipeline,
    "BRANCH_PREFIX",
)
check(
    "The pipeline allows five rework cycles by default",
    "DEFAULT_MAX_CYCLES: u32 = 5" in pipeline,
    "DEFAULT_MAX_CYCLES",
)
models = read("crates/fsrs-acp/src/models.rs")
check(
    "The forbidden model is the one that the code filters",
    "local/oxcoder-9b" in models,
    "FORBIDDEN_MODELS",
)

# --- The dashboard ----------------------------------------------------
app = read("crates/fsrs-app/src/app.rs")
check(
    "The dashboard keeps two ratings in flight",
    "MAX_CONCURRENT_ASSESSMENTS: usize = 2" in app,
    "MAX_CONCURRENT_ASSESSMENTS",
)

# --- The benchmark ----------------------------------------------------
report = json.loads(read("docs/benchmarks/decision-model.json"))
check(
    "The report of the benchmark holds the commits that it claims",
    report["commits"] == 60 and report["failed"] == 0 and len(report["rows"]) == 60,
    f"commits={report['commits']} failed={report['failed']}",
)
check(
    "The distribution of the report matches the measured one",
    report["criticality"] == {"High": 2, "Low": 16, "Medium": 40, "Trivial": 2}
    and report["complexity"] == {"Easy": 45, "Medium": 15},
    f"criticality={report['criticality']} complexity={report['complexity']}",
)

# --- The files of the documentation -----------------------------------
for relative in (
    "LICENSE",
    "README.md",
    "docs/README.md",
    "docs/adr/README.md",
    "docs/design/README.md",
    "docs/guides/README.md",
    "docs/benchmarks/README.md",
    "docs/images/hero.png",
    "docs/images/architecture.svg",
    "docs/images/benchmark-distribution.svg",
):
    path = ROOT / relative
    check(
        f"The file {relative} exists",
        path.exists() and path.stat().st_size > 0,
        f"{path.stat().st_size if path.exists() else 0} bytes",
    )

# --- The links of the documentation -----------------------------------
broken = []
checked = 0
for path in sorted(ROOT.rglob("*.md")):
    if "target" in path.parts:
        continue
    for match in re.finditer(r"\[[^\]]*\]\(([^)#]+)(#[^)]*)?\)", path.read_text()):
        target = match.group(1)
        if target.startswith(("http://", "https://", "mailto:")):
            continue
        checked += 1
        if not (path.parent / target).resolve().exists():
            broken.append(f"{path.relative_to(ROOT)} -> {target}")
check(
    "Every link of the documentation resolves",
    not broken,
    f"{checked} links checked" + ("" if not broken else f", broken: {broken}"),
)

# --- The result -------------------------------------------------------
failed = 0
for question, ok, evidence in CHECKS:
    print(("PASS " if ok else "FAIL ") + question)
    print("      " + evidence)
    if not ok:
        failed += 1

print()
print(f"{len(CHECKS) - failed}/{len(CHECKS)} checks passed")
sys.exit(1 if failed else 0)
