# Rust port of Laya — implementation plan

A pure-Rust port of the [Laya](https://github.com/NandhaKishorM/laya) System-1 decision
engine, embeddable as a library plus a compatible HTTP server. Inference runs natively on
[candle](https://github.com/huggingface/candle) (no Python, no ONNX Runtime, no C++ deps).

---

## 1. What Laya is (the part we're porting)

Given a **state** (a string, a JSON object, or a conversation list) and a set of **typed
questions**, Laya scores every question in a **single non-autoregressive forward pass** — no
token generation. Three question types:

- `choice` — pick one label from N (`criteria` is a `label -> description` map or a list of labels).
- `score` — an ordinal 0..N-1 (`criteria` is a list of level descriptions); returns an expected value.
- `noul` — boolean; returns `P(true)`.

The model is a **bidirectional encoder** + a small **decision head**:

- **encoder**: ModernBERT-large (English & typed-decisions checkpoints) or mmBERT-base
  (multilingual checkpoint). Both are the ModernBERT architecture (RoPE, alternating
  global/local sliding-window attention, GeGLU, bias-free, pre-norm).
- **head**: `type_emb` (3×d) added to every position → a 2-layer pre-norm
  `TransformerEncoder` → gather the option "marker" positions → `scorer` MLP → per-option
  logits; plus an `act_head` MLP producing an auxiliary "action" probability.

A **Router** picks the checkpoint per request using dependency-free **script/language
detection**. Helpers: **email cleaning**, canned **question presets**, an embedding
**shortlist** for very large choice sets.

### Reference implementations we mirror
- **Python** (`laya/`): source of truth for behavior.
- **TypeScript** (`laya-ts/`): an *official* non-Python port that produces identical answers.
  It proves the whole thing works without torch and shows exactly which logic is needed. Our
  Rust module layout mirrors it closely. (Difference: laya-ts runs the model via ONNX Runtime;
  we run it natively via candle.)

---

## 2. Scope

**In (this port):**
- Core inference `Agent`: tokenize → forward (candle) → decode typed answers; single-state
  `predict`/`system_one` **and** `predict_batch`.
- `Router` + language/script detection.
- Email cleaning, question presets, embedding shortlist.
- HTTP server: `POST /v1/systemone` + `GET /health` (Jev wire-compatible), env-var config,
  optional bearer auth.

**Out (deliberately skipped):**
- `fast.py` / `tl_kernels.py` — CUDA-only TileLang GEMM kernels (an optional speedup; candle's
  CUDA/Metal backends cover acceleration).
- `mcp/*` — MCP server.
- `integrations/langchain.py` — LangChain/LangGraph glue.
- `cli.py` — thin CLI (a small Rust `clap` binary can be added later; not core).
- Training/eval/benchmarks/research/notebooks.

---

## 3. Crate layout

A Cargo **workspace** so the engine stays dependency-light and the server/CLI are optional:

```
laya-rs/
├── Cargo.toml                     # [workspace]
├── crates/
│   ├── laya/                      # the embeddable library
│   │   ├── src/
│   │   │   ├── lib.rs             # re-exports; public API
│   │   │   ├── common.rs          # serialize_state, render_options, build_sequence,
│   │   │   │                      #   collate, softmax, confidence, temperature helpers
│   │   │   ├── tokenizer.rs       # HF `tokenizers` wrapper + special-id resolution
│   │   │   ├── model.rs           # candle DecisionModel: encoder + head forward
│   │   │   ├── agent.rs           # Agent: load, predict, predict_batch, decode
│   │   │   ├── lang.rs            # script/language detection
│   │   │   ├── router.rs          # Router, RouteDecision, LRU model cache
│   │   │   ├── email.rs           # clean_email_body, email_state
│   │   │   ├── presets.rs         # triage/email/guard/moderation/router question sets
│   │   │   ├── shortlist.rs       # predict_shortlist, embed_fn_from_agent
│   │   │   └── error.rs           # error types
│   │   └── tests/                 # parity tests vs golden vectors (Section 9)
│   ├── laya-serve/                # the HTTP server binary
│   │   └── src/main.rs            # axum app: /v1/systemone, /health
│   └── laya-cli/                  # (optional, later) clap binary mirroring cli.py
└── xtask or scripts/              # golden-vector generation, model fetch helpers
```

**Key dependencies**
- `candle-core`, `candle-nn`, `candle-transformers` — native inference (ModernBERT lives in
  candle-transformers; we add the head on top).
- `tokenizers` (HF, pure Rust) — loads `tokenizer.json` directly; replaces `AutoTokenizer` and
  the hand-rolled BPE in laya-ts.
- `hf-hub` — download checkpoints from Hugging Face with local-cache reuse.
- `safetensors` (via candle) — weight loading.
- `serde` / `serde_json` — config, state serialization, wire types.
- server: `axum` + `tokio` (async surface, blocking inference offloaded to a worker).
- `thiserror`, `anyhow`.

---

## 4. Inference backend (candle) — the hard part

The one non-mechanical piece. Everything else is pure logic.

### 4.1 Encoder
Load ModernBERT via `candle-transformers` from the checkpoint's `encoder/config.json` +
`model.safetensors` (encoder weights are under the `encoder.` prefix). mmBERT is the same
architecture with a larger multilingual vocab, so the same model code loads it given its config.

> **Phase-0 spike (de-risking):** confirm `candle-transformers` exposes ModernBERT and that it
> loads both `convaiinnovations/laya` (ModernBERT-large) and `-multilingual` (mmBERT-base) and
> reproduces `last_hidden_state` within tolerance. If a gap exists (e.g. an unsupported RoPE/
> local-attention detail), fall back to vendoring a small ModernBERT module in `model.rs`.

### 4.2 Decision head — reimplement in candle (`model.rs`)
Mirror `DecisionModel.forward` (`laya/common.py`) and the `_HeadOnly` export
(`laya-ts/scripts/export_onnx.py`), which is the authoritative inference-time head:

```
h = encoder(input_ids, attention_mask).last_hidden_state          # [B, S, d]
h = h + type_emb(qtype)[:, None, :]                                # broadcast add
for layer in head.layers:                                         # 2 pre-norm encoder layers
    h = layer(h, key_padding_mask = attention_mask == 0)
m = gather(h, marker_pos)                                          # [B, K, d] option positions
logits = scorer(m).squeeze(-1)                                     # [B, K]
logits = logits.masked_fill(!marker_mask, -1e4)
# action head features (all from logits over the K markers):
p   = softmax(logits, -1)
k   = clamp(marker_mask.sum(-1), min=2)
ent = -(p * log(clamp(p,1e-9))).sum(-1) / log(k)
top2 = top-2 of p                                                  # (single-marker: pad 2nd = 0)
feats = stack[ top2[:,0], top2[:,0]-top2[:,1], ent, k/255 ]        # [B, 4]
act_logits = act_head( cat[ h[:,0], feats ] )                     # [B, n_act]
return logits, act_logits
```

**Submodule shapes & weight keys** (from `model.safetensors`, exact PyTorch names to map by hand):

| Component | Keys | Notes |
|---|---|---|
| `type_emb` | `type_emb.weight` `[3,d]` | added to all positions |
| head layer i (`nn.TransformerEncoderLayer`, `norm_first=True`) | `head.layers.{i}.self_attn.in_proj_weight` `[3d,d]`, `.in_proj_bias` `[3d]`, `.self_attn.out_proj.{weight,bias}`, `.linear1.{weight,bias}` `[4d,d]`, `.linear2.{weight,bias}` `[d,4d]`, `.norm1.{weight,bias}`, `.norm2.{weight,bias}` | `nhead = d//64`, FFN **activation = ReLU** (torch default — *not* GELU), dropout=0 at eval |
| `scorer` | `scorer.0.{weight,bias}` (LayerNorm), `scorer.1.{weight,bias}` (Linear d→d), `scorer.3.{weight,bias}` (Linear d→1) | index 2 is GELU (no params) |
| `act_head` | `act_head.0.{weight,bias}` (Linear d+4→256), `act_head.2.{weight,bias}` (Linear 256→n_act) | index 1 is GELU; `n_act = len(act_costs)+1` |

Pre-norm layer forward (eval): `x = x + SA(norm1(x)); x = x + linear2(relu(linear1(norm2(x))))`,
`in_proj` splits into Q/K/V (`[3d,d]` → three `[d,d]`), MHA with `nhead` heads, `key_padding_mask`
excludes padding.

### 4.3 Numerics / parity target
- Python's CPU path runs **fp32** (autocast only on CUDA); the ONNX export verifies torch-vs-onnx
  within **1e-4** at fp32. **Target fp32 on candle's CPU backend** and assert answer parity to
  4 decimal places (the precision Laya rounds to). GPU (CUDA/Metal via candle) is an
  opt-in follow-up, not a parity gate.
- Softmax/temperature scaling for the *final* answer is done in `agent.rs` (per question, over the
  k valid markers, `logits/temperature`), exactly as `agent.py._decode_answers` — **not** inside
  the model. The head's internal softmax is only for the action features.

---

## 5. Module-by-module porting map

All of these are pure logic; the risk is behavioral fidelity, mitigated by golden vectors (§9).
The laya-ts equivalents are near-line-for-line and are the primary reference.

| Rust module | Python source | TS reference | Notes |
|---|---|---|---|
| `common.rs` | `common.py` | `common.ts` | `serialize_state` must match Python `json.dumps(ensure_ascii=False, separators=(", ", ": "))` byte-for-byte (see `pyJson` in common.ts). `build_sequence` budgeting (head_max_len, 48-token option cap, left/right truncation) is fiddly — copy exactly. `confidence_from_probs`, `temp_bucket`, `clamp_temperature`, `collate_items`. |
| `tokenizer.rs` | `agent.py::_load_tokenizer` | `tokenizer.ts` | Use HF `tokenizers` crate to load `tokenizer.json` (handles both byte-level BPE for ModernBERT and metaspace/SentencePiece for mmBERT). Resolve `cls/sep/mask/pad/unk` ids + `mask_token` string via `added_tokens` with the alias order in `SPECIAL_ALIASES`. `encode(text, add_special_tokens=false)`. |
| `model.rs` | `common.py::DecisionModel`,`build_model` | export_onnx `_EncoderOnly`/`_HeadOnly` | §4. |
| `agent.rs` | `agent.py` | `agent.ts` | `_check_question` validation messages, `_to_internal` normalization, `_encode_state`, `_decode_answers`, `predict_batch` (multi-state collate) and `system_one = predict_batch([state])[0]`. Skip TileLang `accelerate`. |
| `lang.rs` | `lang.py` | `lang.ts` | Port `_SCRIPT_RANGES`, `_STOP` word lists, `_NON_EN_DIACRITICS`, `_SHARED_WORDS`, regexes (`_WORD`, `_IDENTIFIER`), `detect_script`, `script_profile`, `_non_latin_words`, `latin_profile`, `analyse`, `is_english`. Unicode: use `unicode-general-category` / char ranges; combining-mark check (`unicodedata.combining`). |
| `router.rs` | `router.py` | `router.ts` | `DEFAULT_MODELS`, aliases, `normalise_name`, typed-decisions workflow signatures, `_english_from_code`, full `route` precedence ladder, LRU `max_loaded` cache of agents, `preload`/`unload`/`attach`, context-manager equivalents (Drop). |
| `email.rs` | `email.py` | `email.ts` | Regex-heavy (quote headers, signatures, device footers, disclaimers, EN/PT/ES). Port regexes with the `regex` crate; verify case-insensitivity + multiline flags. |
| `presets.rs` | `presets.py` | `presets.ts` | Static question sets; return `serde_json::Value` / typed structs. |
| `shortlist.rs` | `shortlist.py` | `shortlist.ts` | `shortlist_choice`, `predict_shortlist`, `embed_fn_from_agent` (mean-pooled encoder embeddings, cosine top-k, merge-sort stable order). |

**Public API sketch** (`lib.rs`):
```rust
pub struct Agent { /* ... */ }
impl Agent {
    pub fn load(model_id_or_path: &str, opts: LoadOptions) -> Result<Agent>;
    pub fn predict(&self, state: &State, questions: &Questions) -> Result<SystemOneResult>;
    pub fn predict_batch(&self, states: &[State], questions: &Questions, batch_size: Option<usize>) -> Result<Vec<SystemOneResult>>;
}
pub struct Router { /* ... */ }
impl Router {
    pub fn new(opts: RouterOptions) -> Result<Router>;
    pub fn route(&self, state: &State, questions: Option<&Questions>, hints: RouteHints) -> RouteDecision;
    pub fn predict(&self, state: &State, questions: &Questions, hints: RouteHints) -> Result<SystemOneResult>;
}
// State = String | serde_json::Value | Vec<...>; Questions = ordered map (IndexMap) to preserve order.
```
> Use `IndexMap` for questions/criteria so choice-key order (which determines `argmax` label
> and probability ordering) matches Python dict insertion order.

---

## 6. Model & weight distribution

- **Config**: `rl_agent_config.json` keys we read: `encoder`, `head_layers`, `act_costs`,
  `max_len`, `head_max_len`, `temperature` (list of 3), `temperature_by_options` (map),
  `amp_dtype` (informational; we run fp32). Encoder arch config is `encoder/config.json`.
- **Fetch**: `hf-hub` downloads `rl_agent_config.json`, `model.safetensors`, `tokenizer/*`,
  `encoder/*` from `convaiinnovations/laya` (root = English; `subfolder="multilingual"` /
  `"typed-decisions"`). Cache reuse matches Python behavior.
- **Local path**: accept a directory containing those files (offline embedding).
- No ONNX export needed — we load the original `.safetensors` directly, which removes the
  Python-in-the-loop step that laya-ts requires.

---

## 7. HTTP server (`laya-serve`)

Mirror `serve.py` on `axum` + `tokio`:
- `POST /v1/systemone` — body `{state, questions, model?}` → `router.predict(...)`, response
  is the Jev-shaped `{model, answers, usage, routing}`. Errors → 422; missing `questions` → 400.
- `GET /health` → `{status, loaded, device}`.
- Optional `Authorization: Bearer <LAYA_API_KEY>`.
- Env config: `LAYA_HOST`, `LAYA_PORT`, `LAYA_DEVICE`, `LAYA_PRELOAD`, `LAYA_MODELS`,
  `LAYA_THREADS`, `LAYA_AUTO_TASK`, `LAYA_API_KEY`, `LAYA_LOG_LEVEL`.
- Inference is blocking/CPU-bound → run on `tokio::task::spawn_blocking` with a single-permit
  semaphore (one forward pass at a time), matching serve.py's single-worker executor + lock.
- `_resolve_model` mapping (published HF ids + aliases → checkpoint, else auto-route).

---

## 8. Concurrency & lifecycle notes
- `Agent` holds a candle model + tokenizer; make `predict` take `&self` (candle tensors are
  created per-call). Guard checkpoint *loading* in the Router with a `Mutex`/`RwLock`; leave
  *inference* unguarded so callers can share a checkpoint (as Python does).
- LRU eviction (`max_loaded`, default 2) drops the least-recently-used `Agent`; `Drop` frees
  candle device memory.

---

## 9. Parity testing strategy (the backbone of the port)

Behavioral fidelity is the whole game. Two layers:

1. **Pure-logic golden vectors (no model, deterministic).** Add a Python script
   (`scripts/dump_golden.py`) that imports the upstream `laya` package and dumps JSON fixtures
   for a spread of inputs:
   - `lang.analyse` / `is_english` over a multilingual corpus (English, Romance ASCII-stripped,
     CJK, Indic, romanized Bangla/Azerbaijani, mixed brand-name states, identifiers/URLs).
   - `router.route` decisions (with/without hints, workflow signatures).
   - `clean_email_body` over EN/PT/ES samples (quotes, signatures, footers, disclaimers).
   - `common.build_sequence` token id sequences + marker positions for each question type and
     the truncation edge cases (needs the real tokenizer, so keyed per checkpoint).
   - `render_options`, `confidence_from_probs`, `temp_bucket`, `serialize_state`.
   Rust tests assert byte/か value equality against these fixtures. (laya-ts's `tests/` —
   `lang.test.ts`, `router.test.ts`, `tokenizer-parity.test.ts`, `parity-sequence.test.ts`,
   `scaffold.test.ts` — enumerate the exact cases to cover; port them.)

2. **End-to-end model parity (needs weights).** `scripts/dump_e2e.py` runs the real Python
   `Agent.system_one` / `predict_batch` on the presets over a handful of states per checkpoint and
   dumps full answer JSON. Rust asserts equality of `choice`, `score` (±1e-4), `noul` (±1e-4),
   `confidence`, and `act_probability` (±1e-4). This is the gate that certifies the candle
   encoder+head matches torch. Run in CI behind a feature flag / cached weights (checkpoints are
   ~322–421M params).

Also port laya-ts's `email-presets.test.ts`, `bpe.test.ts`, `shortlist.test.ts`, `agent.test.ts`
(the last with a fake provider) as unit tests.

---

## 10. Milestones

- **M0 — spike (½–1 day).** Confirm candle-transformers loads ModernBERT + mmBERT and matches
  `last_hidden_state`. Decide load-vs-vendor for the encoder. *Gate for the whole approach.*
- **M1 — pure logic.** `common`, `lang`, `router`, `email`, `presets` + their golden-vector
  tests. No model needed; fully verifiable offline. Highest value / lowest risk.
- **M2 — tokenizer.** `tokenizer.rs` + `build_sequence` parity against per-checkpoint fixtures.
- **M3 — model.** Encoder load + head reimplementation + weight mapping; `last_hidden_state` and
  `logits`/`act_logits` parity on fixed inputs.
- **M4 — agent.** `predict` / `predict_batch` / decode; end-to-end answer parity on presets.
- **M5 — router integration.** LRU cache, `Router.predict` wiring, multi-checkpoint parity.
- **M6 — shortlist.** `predict_shortlist` + `embed_fn_from_agent`.
- **M7 — server.** `laya-serve` axum app + health/auth + a smoke test against the Python server's
  responses.
- **M8 — polish.** Docs, examples, optional GPU (candle CUDA/Metal), optional `laya-cli`.

---

## 11. Risks & open questions

- **candle ModernBERT coverage** (M0) — biggest unknown; mitigated by the spike and a vendored
  fallback. mmBERT's exact config (tokenizer = Gemma-style metaspace, ~256k vocab) must load too.
- **Head FFN activation** — torch `TransformerEncoderLayer` defaults to **ReLU**; easy to
  wrongly assume GELU. Pinned in §4.2.
- **`serialize_state` / JSON formatting parity** — Python `json.dumps` spacing and non-ASCII
  handling must be reproduced exactly (affects token ids). Covered by golden vectors; see
  `pyJson` in common.ts for the precise rules.
- **Dict/insertion order** — choice label order drives outputs; use `IndexMap` everywhere.
- **Unicode edge cases in `lang.rs`** — `str.isalpha`, `isupper`, combining marks, fullwidth
  Latin ranges. Use `unicode-*` crates and test against the fixtures.
- **fp32 tolerance** — assert to 4 dp (Laya's rounding); flag if any case exceeds it.
- **Weights are large** — E2E parity tests need cached checkpoints; keep them behind a feature/CI
  cache, not a default `cargo test`.

## 12. Open questions for you
1. **GPU**: CPU-fp32 first (parity gate), candle CUDA/Metal as a later opt-in — OK? Or is GPU
   required in v1?
2. **`laya-cli`**: include the small `clap` CLI mirroring `cli.py`, or library + server only?
3. **License/attribution**: upstream is Apache-2.0 — carry the license + NOTICE and attribute the
   port. Assume yes unless you say otherwise.
4. **Publish target**: is this destined for crates.io (naming, semver) or an internal embed?
```
