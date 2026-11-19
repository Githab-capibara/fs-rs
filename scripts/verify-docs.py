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
client = read("crates/fsrs-ai/src/client.rs")
check(
    "The default timeout of the client is the documented one",
    "timeout: Duration::from_secs(120)" in client,
    "the default timeout of FoqClient::new",
)
check(
    "The client asks the server how many requests it serves at the same time",
    "pub fn slots(&self) -> usize" in client and '"total_slots"' in client,
    "FoqClient::slots",
)
check(
    "A request that lost its connection is repeated",
    "fn is_a_closed_connection" in client and "the request is repeated" in client,
    "the retry of the completion request",
)
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
    "The dashboard keeps two ratings in flight at most, and never more than the slots of the server",
    "MAX_CONCURRENT_ASSESSMENTS: usize = 2" in app
    and "fn concurrent_assessment_limit(slots: usize) -> usize" in app,
    "MAX_CONCURRENT_ASSESSMENTS and the limit of the slots",
)
main = read("crates/fsrs-app/src/main.rs")
check(
    "The allocator of the program is the one that ADR-11 promised",
    "#[global_allocator]" in main and "mimalloc::MiMalloc" in main,
    "the allocator of the program",
)
check(
    "The table of the commits draws only the rows that are visible",
    "show_rows" in app and "const ROW_HEIGHT: f32" in app,
    "the virtualisation of the table",
)
check(
    "The header of the columns and the rows share the same widths",
    "fn commit_columns" in app and "fn commit_header" in app,
    "the widths of the columns of the table",
)
check(
    "The frames of the window are measured",
    "FSRS_CONTINUOUS_REPAINT" in app and "fn record_frame" in app,
    "the measurement of the frames",
)
check(
    "The window is drawn when something happened, and not while nothing happens",
    "fn handle_messages(&mut self) -> bool" in app
    and "changed || self.work_in_flight() != before" in app
    and "request_repaint_after(std::time::Duration::from_millis(250))" in app,
    "the decision to ask for a frame",
)
gui = read("crates/fsrs-app/tests/gui.rs")
check(
    "The tests of the window watch when it asks for a frame",
    "fn the_window_asks_for_a_frame_only_when_something_happens" in gui
    and "has_requested_repaint" in gui,
    "the test of the frames of an idle window",
)

# --- The state of the window --------------------------------------------
state = read("crates/fsrs-app/src/state.rs")
check(
    "The state of the window keeps what it derives",
    "pub struct CommitRow" in state
    and "Arc<[usize]>" in state
    and "struct Filtered" in state,
    "the rows of the commits, the filter of the picker and the lines of the log",
)

# --- The git layer ----------------------------------------------------
git = read("crates/fsrs-core/src/git.rs")
check(
    "The options of the performance of git2 are turned off",
    "enable_caching(false)" in git and "strict_hash_verification(false)" in git,
    "configure_performance",
)
check(
    "The metadata of a commit can be read without a diff",
    "pub fn commit_meta" in git and "pub fn pending_commit_metadata" in git,
    "the split of the metadata and the statistics of a commit",
)
check(
    "The lines of a diff are counted in one walk of the diff",
    "Some(&mut |_delta, _hunk, line|" in git and "diff.stats()" not in git,
    "the counting of the lines of a commit",
)
check(
    "The statistics of the commits are moved, not copied",
    "known.remove(&sha)" in git and "pub fn stats_of" in git,
    "the map of the statistics",
)

# --- The store --------------------------------------------------------
store = read("crates/fsrs-core/src/store.rs")
check(
    "The store writes only the rows that really changed",
    "fn stored_shas" in store
    and "fn confirm_pending_count" in store
    and "json_each" not in store,
    "the write of the list of the pending commits",
)
check(
    "The counter of the writes of the store is part of its interface",
    "pub fn total_changes" in store,
    "the counter of the writes",
)
check(
    "The runs of a repository are indexed",
    "idx_runs_repo" in store,
    "the index of the runs",
)
check(
    "The events of a run and the state of the run are written together",
    "pub fn insert_run_events_with_state" in store
    and "unchecked_transaction" in store,
    "the transaction of the events of a run",
)

# --- The serializers --------------------------------------------------
ai_manifest = read("crates/fsrs-ai/Cargo.toml")
core_manifest = read("crates/fsrs-core/Cargo.toml")
client = read("crates/fsrs-ai/src/client.rs")
check(
    "The typed bodies of the decision model are written with sonic-rs",
    "sonic_rs::to_string(payload)" in client
    and "sonic_rs::from_str(&body)" in client,
    "the body of a completion",
)
check(
    "The small untyped answers of the runtime stay with serde_json",
    "serde_json::from_str::<serde_json::Value>(&body)" in client,
    "the answer of the tokenizer",
)
check(
    "The core crate does not depend on the untyped serializer",
    "serde_json" not in core_manifest and "sonic-rs" in core_manifest,
    "the dependencies of the core crate",
)
check(
    "The HTTP client of the decision model carries no serializer of its own",
    'ureq = { version = "3", default-features = false }' in ai_manifest,
    "the features of the HTTP client",
)
check(
    "The two serializers are measured against each other",
    "write_the_request_with_sonic" in read("crates/fsrs-ai/benches/json.rs")
    and 'name = "json"' in ai_manifest,
    "the benchmark of the serializers",
)

# --- The worker -------------------------------------------------------
worker = read("crates/fsrs-app/src/worker.rs")
check(
    "The worker keeps the statistics of the commits in memory",
    "known: Arc<Mutex<HashMap<i64, HashMap<String, CommitStats>>>>" in worker,
    "the cache of the statistics",
)
check(
    "The background writes to the database and the window does not",
    "pub fn with_store" in worker and "store: Option<Arc<Mutex<Store>>>" in worker,
    "the connection of the worker",
)
check(
    "The worker writes the events of a run in one batch",
    "insert_run_events_with_state" in worker and "const BATCH: usize" in worker,
    "the batch of the events of a run",
)

# --- The live tests ---------------------------------------------------
live_server = read("crates/fsrs-ai/tests/live_server.rs")
check(
    "The live tests of the decision model take the single slot of the server",
    "static SLOT: LazyLock<Mutex<()>>" in live_server,
    "the lock of the live tests",
)
live_opencode = read("crates/fsrs-acp/tests/live_opencode.rs")
check(
    "The live tests of the agent take the agent one after another",
    "static AGENT: LazyLock<Mutex<()>>" in live_opencode,
    "the lock of the live tests of the agent",
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

# --- The build ---------------------------------------------------------
build = read("scripts/build-pgo.sh")
check(
    "The build with a profile can apply BOLT to the binary",
    '--bolt' in build and "FSRS_PGO_BOLT_DATA" in build and "llvm-bolt" in build,
    "the BOLT step of scripts/build-pgo.sh",
)
check(
    "The training of the profile rates the commits of a real repository",
    "examples/benchmark" in build and "FSRS_PGO_REPOSITORY" in build,
    "the default training command of scripts/build-pgo.sh",
)
check(
    "The profile of BOLT is recorded from a real run of the dashboard",
    "branches:u" in read("scripts/record-bolt-profile.sh")
    and "perf2bolt" in read("scripts/record-bolt-profile.sh"),
    "scripts/record-bolt-profile.sh",
)
check(
    "The start of the program is measured with interleaved runs",
    "interleaved" in read("scripts/measure-startup.py"),
    "scripts/measure-startup.py",
)
check(
    "The listing of the missing commits is measured against a real fork",
    "sync finished" in read("scripts/measure-sync.sh"),
    "scripts/measure-sync.sh",
)
check(
    "The frames of the window are measured against a real fork",
    "FSRS_CONTINUOUS_REPAINT" in read("scripts/measure-frames.sh")
    and "the frames of the window" in read("scripts/measure-frames.sh"),
    "scripts/measure-frames.sh",
)
check(
    "The size of the code of every crate is measured",
    "cargo llvm-lines" in read("scripts/measure-lines.sh"),
    "scripts/measure-lines.sh",
)
benchmarks = read("docs/benchmarks/README.md")
check(
    "The benchmarks hold the measured size of the code of every crate",
    "| `fsrs-core` | 85 579 |" in benchmarks
    and "| `fsrs-ai` | 57 229 |" in benchmarks
    and "| `fsrs-acp` | 225 934 |" in benchmarks
    and "| `fsrs-app` | 79 033 |" in benchmarks,
    "the table of the lines of LLVM",
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
    "crates/fsrs-core/README.md",
    "crates/fsrs-ai/README.md",
    "crates/fsrs-acp/README.md",
    "crates/fsrs-app/README.md",
    "crates/fsrs-ai/benches/json.rs",
    "deny.toml",
    ".github/workflows/ci.yml",
    "scripts/build-pgo.sh",
    "scripts/record-bolt-profile.sh",
    "scripts/measure-startup.py",
    "scripts/measure-sync.sh",
    "scripts/measure-frames.sh",
    "scripts/measure-lines.sh",
):
    path = ROOT / relative
    check(
        f"The file {relative} exists",
        path.exists() and path.stat().st_size > 0,
        f"{path.stat().st_size if path.exists() else 0} bytes",
    )

# --- The records of the decisions --------------------------------------
adr_index = read("docs/adr/README.md")
adr_files = sorted(
    path.name for path in (ROOT / "docs/adr").glob("ADR-*.md")
)
missing_adr = [
    name for name in adr_files if f"({name})" not in adr_index
]
check(
    "Every record of a decision is listed in the index of the records",
    adr_files and not missing_adr,
    f"{len(adr_files)} records" + ("" if not missing_adr else f", unlisted: {missing_adr}"),
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
