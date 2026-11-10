# The local decision model

`fs-rs` rates the criticality and the complexity of every upstream commit with
a model that runs on the machine of the user. The commits of a private fork
never leave it.

The model is [Foq](https://github.com/yohanargentina-oss/Foq) — a decision
engine, not a chat model. It answers with a letter, and it returns the
log-probability of every letter, which `fs-rs` calibrates into a confidence.

## The pieces

| Piece | Where |
|-------|-------|
| The runtime | `foq-llama-linux-x64.tar.gz` v1.0.5, the CUDA build of `llama-server` |
| The model | `~/.models/foq/foq-reflex-8b-pq2_0.gguf` (PQ2_0, 2.13 bpw) |
| The decision adapter | `~/.models/foq/foq_decision_8b.gguf` |
| The Python environment | `~/.local/share/foq/venv`, with the CUDA 12 wheels |
| The service | `~/.config/systemd/user/foq-server.service` |

The runtime needs the CUDA libraries of the environment, so the unit carries
the `LD_LIBRARY_PATH`:

```ini
[Service]
Type=simple
Environment=CUDA_VISIBLE_DEVICES=0
Environment=LD_LIBRARY_PATH=%h/.local/bin/foq-llama/foq-llama:%h/.local/share/foq/venv/lib/python3.13/site-packages/nvidia/cuda_runtime/lib:%h/.local/share/foq/venv/lib/python3.13/site-packages/nvidia/cublas/lib:%h/.local/share/foq/venv/lib/python3.13/site-packages/nvidia/cuda_nvrtc/lib
ExecStart=%h/.local/bin/foq-llama/foq-llama/llama-server -m %h/.models/foq/foq-reflex-8b-pq2_0.gguf --lora %h/.models/foq/foq_decision_8b.gguf -ngl 99 -c 4096 -b 2048 -ub 512 -np 1 --flash-attn on --port 8089 --host 127.0.0.1 --alias foq,foq-system1
Restart=on-failure
RestartSec=5
```

Start it:

```bash
systemctl --user daemon-reload
systemctl --user enable --now foq-server.service
```

Check it:

```bash
curl -s http://127.0.0.1:8089/health
curl -s http://127.0.0.1:8089/props | python3 -m json.tool | head
```

## The window of the runtime

`-np 1` is not a detail. The context window is shared between the slots of a
parallel server: with `-c 4096 -np 4` every slot gets 1024 tokens, and the
runtime refuses a prompt of 1380 tokens with

```text
request (1380 tokens) exceeds the available context size (1024 tokens)
```

The dashboard runs at most two ratings at a time, and every rating sends its
two requests one after the other, so one slot is enough — and it gives the
diff four times more room.

The client adapts to whatever the runtime reports, so a runtime with a
different window still works: it asks `/props` for the size and `/tokenize`
for the number of tokens, and it shortens the diff until the prompt fits. See
[ADR-06](../adr/ADR-06-context-fitted-prompts.md).

## The GPU

The decision model owns the discrete NVIDIA GPU. The order of preference is:

1. the discrete NVIDIA GPU (`gpu0`),
2. the integrated Intel GPU,
3. the CPU.

`fsrs-ai::gpu` inspects the machine: it reads the NVIDIA devices through
`nvidia-smi`, the Intel devices through the render nodes in `/dev/dri`, and it
lists the processes that hold the NVIDIA device. A foreign process that holds
the GPU is a problem: the model must not share the device with an unrelated
service.

Check who holds it:

```bash
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits
```

Only `llama-server` of the `foq-server` service should be listed. A retired
`llama-oxcoder` service must stay stopped.

## The calibration

The profile has to match the runtime:

| `foq_profile` | Runtime | Temperature |
|---------------|---------|-------------|
| `lora` | started with `--lora` | 2.6353 |
| `bare` | started without the adapter | 3.6381 |

## Rate a commit from the command line

```bash
cargo run -p fsrs-ai --example benchmark -- /path/to/upstream HEAD 20
```

The report is written to standard output as JSON, one line per commit on
standard error. See the [benchmark report](../benchmarks/README.md).

## The forbidden model

The retired build `OxCoder-9B IQ4_XS` (`local/oxcoder-9b`) is not allowed to
run. Its service must stay stopped, and `fs-rs-acp` filters the identifier out
of the list of models that the user can pick, so it can never be selected for
an agent.
