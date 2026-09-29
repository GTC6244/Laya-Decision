# Changelog

All notable changes to Laya-Decision are documented here. This project ports the upstream
[Laya](https://github.com/NandhaKishorM/laya) engine; each entry notes the upstream version tracked.

## Unreleased

### Added
- **`laya-serve` honours `LAYA_INFERENCE_CONCURRENCY`** (default 1): how many forward passes run
  in parallel. The single-permit gate was a hard-coded `Semaphore::new(1)`, so a burst of requests
  serialised behind one worker no matter how much headroom the host had. Raising it on CPU
  multiplies throughput for a small bounded memory cost — measured 6.8× at 8 permits on a 32-core
  host, for +10% peak RSS. On a GPU the default of 1 should be kept: one pass already saturates the
  device, and extra permits add no throughput while making tail latency worse. This is independent
  of `LAYA_MAX_CONCURRENT` (default 16), which bounds admitted requests and sheds 503.

## 0.2.3 — tracks upstream Laya 0.3.21 (upstream @9d95567)

Ports the upstream `laya/` changes made after the previous sync (upstream @4066d5d) that affect the
Rust surface, up to the 0.3.21 release. The golden parity fixtures were regenerated from upstream
0.3.21; they are byte-identical to the previous set, because the ported behaviour changes do not
alter any sampled case (the language change is a behaviour-preserving fast-path skip, and the
sign-off change only affects sign-off names outside Latin-1).

### Changed
- **Email sign-off detection matches names by Unicode category, not a Latin-1 range.** A closing
  line's trailing name is now recognised when its first letter is uppercase (`Lu`), titlecase
  (`Lt`) or caseless (`Lo`) in any script, matching upstream's new `_is_english_signoff`. The old
  name class excluded only `a-z` and `ß-ÿ`, so an extended-Latin **lowercase** name such as
  `żaneta` was read as a capital and the line `Thanks, żaneta` was dropped as a signature; it is now
  kept, while `Regards, Łukasz` and `Дмитрий`/`山田` closings are still stripped (upstream
  signoff fix).
- **Choice labels in list form must be unique and non-null.** A `null` label is rejected (it would
  render as the text `null` while its answer key is the string `"null"`, so a client cannot tell it
  from the string `"null"`), and two labels that collapse to the same answer key are rejected naming
  the repeat — both previously produced a question that silently scored fewer options than the caller
  wrote. The nested-list/object rejection message now says a label must be “a string, number or
  bool” (upstream #425 duplicate/null-label fix).

### Added
- **`laya-serve` honours `LAYA_MAX_LOADED`** (default 2): the number of checkpoints kept resident at
  once. A value below what routing can choose reloads one per switch; preloading still raises the cap
  to hold whatever it builds, so this never evicts a preloaded checkpoint (upstream `LAYA_MAX_LOADED`).
- **`laya-serve` returns `Retry-After: 1` on a 503.** The admission-control shed now hints when a
  slot is likely free, since admission turns over at inference speed (upstream 503 `Retry-After`).
- **CLI `--model auto`** routes the request (the same as omitting `--model`); other values reach the
  router, which already resolves names and aliases (`en`, `ml`, `td`, …) through `normalise_name`
  (upstream cli `model_name`).

### Performance
- **Language detection skips leaf lines shorter than 7 characters** before the code-line check and a
  full analysis pass. Such a line can never be selected as a state's deciding non-English segment
  (every branch needs four word tokens or ten letters), so this is behaviour-preserving (upstream
  `lang` short-line skip).

### Notes on upstream changes not needing (or not applicable to) a Rust change
- **Opt-in abstention / `min_confidence` and `laya.confidence`** (#361), **`decide_batch`**, the
  **`predict_long` hook threading**, **`predict_batch` `sort_by_length` / per-request token budget**,
  **`Router(agent_kwargs=…)` / per-model `LAYA_SHA256_DIGESTS`** and the batch end-hook ordering
  rework all live on Python surfaces the Rust port does not have: it is single-request with no hooks,
  no batch path, no structured `decide`, and no `agent_kwargs`/digest plumbing.
- **`laya-serve` per-request `max_len`/`head_max_len` and `LAYA_MAX_TOKEN_BUDGET`** (#583) do not
  apply: the Rust `Agent.system_one` has no token-budget override argument, so there is nothing to
  cap. The `usage["options"]` collapse report (#538) has no counterpart for the same reason.
- **`/health` device/`cpu_fallbacks` fields** report torch's silent GPU→CPU per-request fallback;
  the Rust engine uses native candle and has no scoped OOM-fallback counter to surface.
- **The torch/ONNX/TileLang/`torch.compile` work** (dynamic-shape attention for ONNX export, the
  `compile=` option, per-thread duck-shape, temperature-shape validation guarding a Python
  `IndexError`) has no Rust counterpart; the Rust temperature loader always yields three clamped
  values, so the shape guard is moot.
- **`email_state(max_chars=…)`** was left off to keep the public Rust signature stable for a patch
  release; the underlying `clean_email_body_with(body, max_chars)` already exposes the budget.
- **CLI `--questions` / `--batch` / `--batch-size` / `--max-len` / `--head-max-len`** need the batch
  and token-budget surfaces the Rust port does not have; `--preset` already sends the text under the
  field each preset names, matching upstream's new `state_field`.
- The **integrations (LangChain, LlamaIndex, CrewAI), MCP server/tools, `_compile`, `revisions`,
  `hooks`, `onnx_agent`, `evals`** modules are Python-only and out of scope for the Rust port.

## 0.2.2 — tracks upstream Laya main (post-0.3.20, upstream @4066d5d)

Ports the upstream `laya/` changes made after the previous sync (upstream @970dc8c) that affect the
Rust surface. The golden parity fixtures were regenerated from upstream main and gained cases
exercising the new routing behaviour below.

### Changed
- **Loanword rescue for English routing.** A single accented loanword or proper noun (`café`,
  `résumé`, `José`) in an otherwise plain-English sentence used to clear the non-English diacritic
  floor — the rate is measured over every character — and route to the multilingual checkpoint.
  `lang::latin_profile` now keeps such text English when it shows at least two distinct English
  function words that no other list holds and at most one word carrying a non-English letter, and
  the diacritic rate is below the new `ENGLISH_RESCUE_DIACRITIC_RATE` (0.06). A higher rate, more
  than one accented word (Nordic `två gånger`), or fewer English-only words still routes to
  multilingual (upstream #337, #350).
- **Language detection reads every string value in a dict state.** English sibling fields, or an
  English note longer than the 4000-character segment-scan budget, could hide a non-English field
  (e.g. a German customer message) behind them, so the joined detection window read as English and
  routing fell through to the English checkpoint. `lang::analyse` now reads each string value on its
  own after the segment scan and lets one non-English value decide, with the same name/acronym/code
  guards the segment scan uses so a name field cannot pull an English ticket off the checkpoint
  (upstream #384).

### Fixed
- **`laya-serve` rejects a missing state instead of answering about `"null"`.** A request with no
  `state` (or `"state": null`) was answered as a confident decision about the literal text `null`,
  with nothing in the response to show the state was absent. `POST /v1/systemone` now returns 400
  `'state' is required`; a string state (including `"null"` and `""`) is left alone (upstream
  serve-require-state).
- **`laya-serve` bounds answer options.** A choice/score question with an unbounded option count
  amplifies a small request into a large collated tensor. Requests are now capped at 100 choice
  options, 32 score levels and 512 total options across questions, returning 413 (upstream #335).
- **Choice labels must be scalars.** A `choice` question whose list-form `criteria` contains a list
  or object label is rejected, naming the question and label index, instead of silently
  JSON-stringifying it into an answer key (upstream #425). The dict form is unaffected: its keys are
  always strings.

### Added
- **`laya-serve` admission control.** A non-blocking concurrency bound (`LAYA_MAX_CONCURRENT`,
  default 16) admits requests just after auth and holds the slot through the response, shedding
  excess load with 503 rather than buffering many bodies behind the single inference gate (upstream
  #330).
- **CLI `--preset` sends the text under the field the question set names** (`email`→`body`,
  `guard`→`prompt`, `moderation`→`post`, `router`/default→`request`, `triage`→`message`) instead of
  a fixed `text` key none of them reads (upstream fix/cli-preset-state-key). Routing is unaffected:
  `route` reads the state only for key-invariant language detection.

### Notes on upstream changes not needing a Rust change
- The upstream torch/CUDA/XPU/TileLang/ONNX work (CUDA-AMP override, XPU bf16 autocast, per-request
  OOM-fallback scoping, the TileLang fast path dtype, `ONNXAgent` lang threading) has no Rust
  counterpart: the Rust engine uses native candle on CPU/Metal, not torch.
- The async-hooks + per-hook timeout work and the `predict_batch` hook composition (#277, #435) do
  not apply: the Rust `Router` has no process-wide hooks.
- Revision pinning + SHA-256 checkpoint verification (#332, #347) is a Python model-loading feature;
  the Rust loader uses `hf-hub` and has no revision-pinning surface yet.
- The new labelled evaluation harness and `laya-eval` CLI, the `structured` schema changes
  (pydantic Optional/anyOf, enum-collision and multi-type rejection), and the MCP tools
  (`laya_shortlist`, preserved question fields) have no Rust surface.
- `common.py`'s fast-tokenizer thread lock and the `no_init` decision-head build are torch/HF
  specifics: Rust runs one inference at a time behind a single-permit gate and candle loads weights
  directly, so neither changes behaviour here.
- `feat(agent): predict_long` (scan a state past the context window and aggregate per question,
  #363) is a new library capability not yet ported; it is tracked for a future release.

## 0.2.1 — tracks upstream Laya main (post-0.3.20, upstream @970dc8c)

Ports the upstream `laya/` changes made after the 0.3.20 tag that affect the Rust surface. The
golden parity fixtures were regenerated from upstream main and gained cases exercising the new
behaviour below.

### Changed
- **Mixed-language routing.** A mostly-English state can hide the customer's non-English line
  behind a longer English stack trace, error payload or form template. `lang::analyse` now reports
  a new `mixed_segment`: a state that would go to the English checkpoint is checked line by line
  and field by field, and a line that reads as a non-English language on its own (four+ words, two
  distinct stopwords, ignoring code lines, identifiers and acronyms) routes to `multilingual`
  instead. The router reason becomes `"Latin script, mostly English, but a line or field reads as
  … ; the English checkpoint cannot read it"` (upstream #207).

### Fixed
- **`$LANG` values that name no language abstain.** An explicit `lang` (or `lang_guess`) of `C`,
  `POSIX`, `C.UTF-8`, or the ISO 639-2 special codes `und`/`zxx`/`mul` no longer forces the
  `multilingual` checkpoint; like a blank code it falls through to detection, so `LANG=C` on a
  minimal image does not pin every request to one checkpoint (upstream #368).
- **Email `From:` prose vs. reply header.** A `From:` line is treated as a quoted-reply header only
  when an address (`@`/`<`) follows, so ordinary prose opening with `From:` is kept. A bare
  `From: Name` header is still cut when its own `Sent:`/`Date:` line follows (upstream #371).
- **`score` questions reject a null level.** A `score` question whose `criteria` list contains a
  null level is rejected, naming the index, instead of silently dropping that description
  (upstream: reject a null score level).
- **`laya-serve` logs the inference failure the client cannot see.** A failed inference still
  returns a fixed 500 to the client, but the cause is now logged server-side (upstream #375).

### Added
- **`laya-serve` timing headers.** `POST /v1/systemone` responses carry `Server-Timing:
  inference;dur=<ms>` and `X-Inference-Time-Ms: <ms>` (upstream #372).

### Notes on upstream changes not needing a Rust change
- The upstream `_IDENTIFIER` ReDoS fix (a `(?<![\w-])` look-behind, #383) is unnecessary here: the
  `regex` crate is already linear-time and the look-behind removes no match, so the match set is
  identical.
- The non-string `choice` label fix (#380) is a no-op in Rust: `serde_json` object keys are always
  strings.
- The router `predict_batch` hook-composition and `lang_temperatures` forwarding (#379, #381) have
  no Rust counterpart — the Rust `Router` has neither process-wide hooks nor per-language
  temperatures. Deeply-nested-JSON handling (#401) is already covered: axum's JSON extractor
  rejects such bodies with a 4xx before the handler runs, never a 500.

## 0.2.0 — tracks upstream Laya 0.3.20

Ports the upstream changes made in Laya 0.3.11 → 0.3.20 that affect the Rust surface.

### Changed
- **Language detection / routing.** German is now detected from plain-ASCII text via an expanded
  German stopword list, so tickets like `"Mein Konto wurde zweimal belastet"` route to the
  multilingual checkpoint instead of defaulting to English (upstream #130). Golden parity fixtures
  regenerated from upstream 0.3.20.
- **Answers gain `answer_confidence`.** Every `choice`/`score`/`noul` answer now reports both
  `confidence` (unchanged) and `answer_confidence` — the calibrated `max(p)` on every question
  type, the quantity temperature scaling fits (upstream #126).
- **Router: blank `lang` falls through to detection.** An explicit `lang` that is blank/whitespace
  no longer forces `multilingual`; it falls through to `lang_guess`/detection like an abstaining
  hint. Real language codes still route immediately (upstream #292).

### Added
- **CLI `--preset`** answers a ready-made question preset (`email`, `guard`, `moderation`,
  `router`, `triage`) instead of the router questions; implies `--predict` (upstream #303).
- **`laya::answer_confidence`** in the library (mirrors `laya.answer_confidence`).

### Fixed
- **`noul` criteria key validation.** A `noul` question whose `criteria` dict is keyed anything
  other than `true`/`false` is now rejected instead of silently dropping the descriptions
  (upstream #156).
- **Empty `HF_TOKEN` treated as no token**, so no empty `Bearer` header is sent to the Hub
  (upstream #264).
- **`laya-serve` hardening**: request limits (max 64 questions, 50k state chars), an explicit 2 MiB
  body cap, constant-time bearer-token comparison, validated `LAYA_PORT`, and internal errors are
  no longer leaked to clients — validation errors return 422, everything else a generic 500
  (upstream #250, #251, #268, #296).
- **`email`/`lang` bound work before truncation** so a single huge input cannot dominate cleaning
  or detection (upstream #253).

## 0.1.1

Crate metadata fixes.

## 0.1.0

Initial pure-Rust port of Laya: native candle inference, Router, HTTP server, and CLI.
