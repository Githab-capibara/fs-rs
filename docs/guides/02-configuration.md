# Configuration

The configuration lives in `~/.config/fs-rs/config.toml`. Every key has a
default, so a missing key is not an error: the file may hold nothing but the
repositories.

```toml
# The local decision model.
foq_url = "http://127.0.0.1:8089"
foq_model = "foq"
foq_profile = "lora"
min_confidence = 0.8
foq_timeout_secs = 120

# The agents.
acp_command = "opencode acp"
terminal_command = "opencode mini"

# Permissions.
auto_approve = true

# The tracked forks.
[[repos]]
name = "fd"
path = "/home/user/projects/fd"
upstream_url = "https://github.com/sharkdp/fd.git"
upstream_web = "https://github.com/sharkdp/fd"
main_branch = "master"
```

## The decision model

| Key | Default | Meaning |
|-----|---------|---------|
| `foq_url` | `http://127.0.0.1:8089` | Address of the decision server. |
| `foq_model` | `foq` | Alias of the model on that server. The runtime registers `foq` and `foq-system1`. |
| `foq_profile` | `lora` | Calibration profile. `lora` fits the model with the decision adapter, `bare` the model alone. |
| `min_confidence` | `0.8` | Calibrated confidence below which a rating is flagged for review. |
| `foq_timeout_secs` | `120` | Timeout of a single request to the decision server, in seconds. A big diff needs a long time on a slow accelerator. |

`foq_profile` and the runtime have to agree: `lora` belongs to a runtime that
was started with `--lora`, `bare` to one without.

## The agents

| Key | Default | Meaning |
|-----|---------|---------|
| `acp_command` | `opencode acp` | The command that starts the agent for the automated pipeline. It has to speak the Agent Client Protocol. |
| `terminal_command` | `opencode mini` | The command that runs in the manual terminal. The model that the user picked is appended as `--model <provider/model>`. |
| `auto_approve` | `true` | Whether the permission requests of the agents are approved automatically. |

The full `opencode` interface ignores `--model` and the `OPENCODE_CONFIG*`
variables, so the manual terminal uses the minimal interface. See
[ADR-02](../adr/ADR-02-opencode-mini-for-the-manual-terminal.md).

## The repositories

| Key | Meaning |
|-----|---------|
| `name` | The name shown in the strip. |
| `path` | The local working copy of the fork. It has to be a git repository with the upstream remote configured. |
| `upstream_url` | The remote of the original project. |
| `upstream_web` | The address of the upstream project in the browser; the link to every commit is built from it. |
| `main_branch` | The branch of the fork that is compared against the upstream branch. The default is `main`. |

The upstream remote is fetched into `refs/remotes/upstream/<main_branch>`, so
the working copy needs a remote named `upstream`:

```bash
git remote add upstream https://github.com/sharkdp/fd.git
```

The dashboard itself does the fetch, but the remote has to exist.

## The state

The database lives in `~/.local/share/fs-rs/fsrs.db`. It holds the tracked
repositories, the known upstream commits, the ratings and the runs. The path
can be changed with `--data` on the command line.
