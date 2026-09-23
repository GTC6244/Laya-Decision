# Laya-Decision

[![CI](https://github.com/GTC6244/Laya-Decision/actions/workflows/ci.yml/badge.svg)](https://github.com/GTC6244/Laya-Decision/actions/workflows/ci.yml)

A **pure-Rust port** of [Laya](https://github.com/NandhaKishorM/laya) — a multilingual,
non-autoregressive *System-1 decision engine*. Given a **state** (text, JSON, or a conversation
list) and a set of **typed questions**, it scores every question in a **single forward pass** —
no text generation, so nothing to parse and nothing to hallucinate.

Inference runs natively on [candle](https://github.com/huggingface/candle) (no Python, no ONNX
Runtime). The routing, language detection, email cleaning, presets and shortlist logic is
dependency-free.

> Status: **working.** The pure-logic core (language detection, routing, email cleaning,
> presets, sequence construction) is verified byte-for-byte against the upstream Python
> package. The candle backend runs the real `convaiinnovations/laya` checkpoint and matches
> PyTorch end-to-end within **1e-4** (the 4-decimal precision Laya publishes) — see
> [Testing](#testing). Runs on CPU and on **Apple Silicon GPU via Metal** (`--features metal`,
> ~4× faster forward on an M4).

## Question types

- **`choice`** — pick one label of N. Returns the label, per-label probabilities, and calibrated confidence.
- **`score`** — an ordinal 0..N-1. Returns the expected value.
- **`noul`** — boolean. Returns `P(true)`.

## Workspace layout

| crate | what |
|-------|------|
| [`laya`](crates/laya) | the embeddable library — `Agent`, `Router`, language detection, email cleaning, presets, shortlist |
| [`laya-serve`](crates/laya-serve) | a Jev-compatible HTTP server (`POST /v1/systemone`, `GET /health`) |
| [`laya-cli`](crates/laya-cli) | the `laya` command — route or answer a request from the terminal |

The `laya` crate's pure-logic modules build **without** the default `model` feature; enabling it
(on by default) pulls in candle, the Hugging Face tokenizer, and Hub download.

## Usage (library)

```rust
use laya::{Router, RouteHints, triage_questions};
use serde_json::json;

let router = Router::with_defaults()?;
let state = json!({ "message": "I was charged twice, please refund" });
let result = router.predict(&state, &triage_questions(), &RouteHints::default())?;
println!("{}", serde_json::to_string_pretty(&result.to_json())?);
```

The `Router` auto-selects a checkpoint per request (English vs. multilingual) using dependency-free
script/language detection; pass `RouteHints { model: Some("multilingual"), .. }` to force one.

Run the bundled demo (downloads the English checkpoint on first use):

```bash
cargo run --release --example predict
```

### Shortlist (high-cardinality choice)

For a choice question with many labels, rank and keep the top-`k` before scoring. Supply your own
bi-encoder as `embed_fn`, or mean-pool the loaded checkpoint's encoder:

```rust
use laya::{shortlist_choice, embed_fn_from_agent};
let embed = embed_fn_from_agent(&agent, 512, 32);
let top = shortlist_choice(&state, &criteria, &embed, 20, None)?; // top-20 labels
```

## Command line

```bash
laya "I was charged twice, please refund"      # routing decision only (offline, no download)
laya "Refactor this service" --predict         # full answers (downloads the checkpoint)
laya "Mein Konto wurde zweimal belastet" --lang de
laya                                           # interactive mode
```

`--json` prints the raw result; `--model`/`--task`/`--device` force routing/device.

## HTTP server

```bash
LAYA_PORT=8000 LAYA_PRELOAD=1 cargo run --release -p laya-serve
curl -s localhost:8000/v1/systemone -H 'content-type: application/json' -d '{
  "state": {"message": "Mein Konto wurde zweimal belastet"},
  "questions": {"is_urgent": {"type": "noul", "instructions": "Is this urgent?"}}
}'
```

Config is via env vars (`LAYA_HOST`, `LAYA_PORT`, `LAYA_DEVICE`, `LAYA_PRELOAD`, `LAYA_MODELS`,
`LAYA_AUTO_TASK`, `LAYA_API_KEY`, `LAYA_LOG_LEVEL`). The wire shape matches TypeSafe Jev's
`/v1/systemone`, so a Jev client can point `baseUrl` here unchanged.

## Testing

Pure-logic parity against the upstream Python package (no weights needed):

```bash
# regenerate golden fixtures from the upstream package (once):
PYTHONPATH=/path/to/laya python3 scripts/dump_golden.py
# run the fast unit + parity suites:
cargo test -p laya --no-default-features
cargo test -p laya --no-default-features --test parity
```

End-to-end model parity vs. PyTorch (downloads a checkpoint; reference produced by
`scripts/dump_e2e.py` in a `torch`+`laya` env):

```bash
.venv-torch/bin/python scripts/dump_e2e.py --checkpoint english     # writes the reference
cargo test -p laya --release --test e2e -- --ignored --nocapture    # candle vs torch
```

The English checkpoint currently matches to `max |Δ| = 1e-4` across the triage/guard/moderation
presets.

### Apple Silicon (Metal)

```bash
LAYA_DEVICE=metal cargo run --release --features metal --example predict
```

Pass `device: Some("metal".into())` to `LoadOptions` (or `LAYA_DEVICE=metal` to `laya-serve`).

## Attribution & license

Apache-2.0. This is a port of upstream Laya (Apache-2.0); see [`NOTICE`](NOTICE) and
[`reference/UPSTREAM-LICENSE`](reference/UPSTREAM-LICENSE). Model checkpoints are distributed
separately on the Hugging Face Hub under their own license.
