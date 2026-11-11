# `fsrs-core`

The domain of `fs-rs`: repositories, upstream commits, ratings, runs, the
SQLite store and the git operations. No network protocol, no GUI and no model
lives here — the crate is the vocabulary every other crate speaks.

| Module | Content |
|--------|---------|
| [`lib`](src/lib.rs) | `Criticality`, `Complexity`, `UpstreamCommit`, `Repo`, `Assessment`, `Run`, `RunState`, `RunEvent`, `Config`, `GpuPreference`, `FoqProfile` |
| [`git`](src/git.rs) | The git layer: fetching the upstream branch, listing the missing commits, reading the diff, creating the branch and the fast-forward merge |
| [`catalog`](src/catalog.rs) | Which commits a fork is missing, computed from the two references |
| [`store`](src/store.rs) | The `rusqlite` connection: the schema, the upserts and the queries that persist the state of the dashboard |
| [`config`](src/lib.rs) | The `config.toml` reader, with every default and the validation |

| Test suite | What it covers |
|------------|----------------|
| [`tests/git_integration.rs`](tests/git_integration.rs) | The git layer against real temporary repositories: fetching, listing, branching, merging, the CRLF separators of commit messages |
| [`tests/catalog_integration.rs`](tests/catalog_integration.rs) | The catalogue against real repositories, including a fork that carries patches upstream does not have |

## Example

```rust
use fsrs_core::{catalog, git};

// Which commits does the fork miss?
let missing = catalog::missing_commits(&repo_path, "master", 25)?;
```

The crate depends on `git2` with the default features disabled and on the
bundled `rusqlite`, so a workspace build needs no system library beyond the
ones the toolchain and zig provide.

## Related documents

| Document | Purpose |
|----------|---------|
| [ADR-06](../../docs/adr/ADR-06-context-fitted-prompts.md) | The context window of the runtime |
| [Configuration guide](../../docs/guides/02-configuration.md) | Every key of `config.toml`, as read by this crate |
| [Design note — the pipeline](../../docs/design/02-three-agent-pipeline.md) | The branch and the merge, as executed by `git.rs` |
