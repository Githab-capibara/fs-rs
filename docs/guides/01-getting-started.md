# Getting started

## What you need

* Rust 1.88 or newer,
* a working copy of a fork and the address of its upstream project,
* the local decision model running on `127.0.0.1:8089` — see
  [the guide about the decision model](03-local-decision-model.md),
* `opencode` on the `PATH`, for the agents and for the manual terminal.

## Build and run

```bash
cargo run -p fsrs-app
```

The first start creates the configuration file and the database:

| Path | Content |
|------|---------|
| `~/.config/fs-rs/config.toml` | the configuration |
| `~/.local/share/fs-rs/fsrs.db` | the state of the dashboard |

Both can be overridden on the command line:

```bash
cargo run -p fsrs-app -- --config /path/to/config.toml --data /path/to/data
```

## Add a repository

Open `~/.config/fs-rs/config.toml` and describe the fork:

```toml
[[repos]]
name = "fd"
path = "/home/user/projects/fd"
upstream_url = "https://github.com/sharkdp/fd.git"
upstream_web = "https://github.com/sharkdp/fd"
main_branch = "master"
```

* `path` — the local working copy of the fork. It has to be a git repository.
* `upstream_url` — the remote of the original project. It is fetched into
  `refs/remotes/upstream/<main_branch>`.
* `upstream_web` — the address of the upstream project in the browser. It is
  used to build the link to every commit, which the agents receive.
* `main_branch` — the branch of the fork that the upstream commits are
  compared against.

Restart the dashboard. The repository appears in the strip at the top, with
the number of upstream commits that are missing from the fork.

## Read the ratings

Every missing commit is rated by the local decision model:

* the **criticality** on the left — how much the commit matters for the fork,
* the **complexity** on the right — how hard the port is.

The letter is the answer of the model, the badge is the level, the colour is
the severity. A rating whose confidence is below `min_confidence` is flagged
for review.

The ratings are produced in the background. The first rating of a repository
takes a few seconds, because the model has to read the prompt.

## Open a commit in a terminal

Click a commit. The dashboard asks which model the terminal should use, shows
the models that OpenCode has registered, and opens a terminal on the right that
runs `opencode mini --model <provider/model>`.

The terminal is a real terminal: the OpenCode CLI behaves as it would in a
shell of its own.

## Port a commit

Click `РЕАЛИЗОВАТЬ` on a commit. The dashboard asks for the model of the
implementation, then:

1. creates the branch `fsrs/implement/<short-sha>`,
2. asks the implementer to port the change,
3. asks the reviewer to run the linters and to fix what they report,
4. asks the verifier to approve or to reject,
5. repeats steps 3 and 4 with the feedback of the verifier until it approves,
6. merges the branch into `main`.

The log at the bottom shows every step. The branch is kept after the merge.

## Where to go next

* [Configuration](02-configuration.md) — every key, with its default.
* [The local decision model](03-local-decision-model.md) — the runtime, the
  GPU and the calibration.
* [The design notes](../design/README.md) — how the parts are built.
