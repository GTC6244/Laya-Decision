# Changelog

All notable changes to Laya-Decision are documented here. This project ports the upstream
[Laya](https://github.com/NandhaKishorM/laya) engine; each entry notes the upstream version tracked.

## 0.2.2 — tracks upstream Laya main (post-0.3.20, upstream @4066d5d)

Ports the upstream `laya/` changes made after @970dc8c that affect the Rust surface. Golden parity
fixtures were regenerated from upstream main and gained cases (plus a new `lang_structured.json`)
exercising the routing changes below.

### Changed
- **Loanword rescue in language detection.** One accented loanword or proper noun (`café`,
  `résumé`, `José`) no longer pulls otherwise-plain English off the English checkpoint. The
  diacritic rate is measured over every character, so a single `é` in a short sentence used to
  clear the non-English floor; `lang::latin_profile` now keeps English when it shows at least two
  distinct function words no other list holds, carries at most one word with a non-English letter,
  and stays under a higher rescue rate (0.06). A vocabulary of several distinct accented words is
  still not rescued (upstream #337).
- **Mixed-language routing reads every string value.** A short non-English field could hide behind
  a long English one: joining every value into one 4000-char detection window let a long English
  note fill the budget, or out-vote a short German message, and that message was then sent to the
  English checkpoint. `lang::analyse` now, after the whole-state and per-line/-field scan, reads
  each string value on its own — one non-English value is enough — so a value the joined window and
  the segment scan cannot reach (a field past the 4000-char cap, or a non-Latin script too small a
  fraction of the join to reclassify) still routes to `multilingual` (upstream #384).

### Fixed
- **`laya-serve` rejects a request with no state.** `serialize_state(null)` is the four characters
  `null`, so a body with no `state` key (or an explicit `"state": null`) was answered as a decision
  about the literal text "null" — HTTP 200, indistinguishable from a real string. It now returns
  400 `'state' is required` before serialization (upstream: require a state on the HTTP surface).
- **`laya-serve` bounds per-request option counts.** A single request packing thousands of choice
  options or score levels tokenizes and collates into one large tensor. `POST /v1/systemone` now
  rejects (413) a `choice` question with more than 100 options, a `score` question with more than
  32 levels, or more than 512 answer options across all questions (upstream: scope option budgets
  to HTTP serving).
- **`laya-serve` bounds concurrent requests.** A new `LAYA_MAX_CONCURRENT` (default 16) caps the
  requests admitted past auth at once; excess is refused with 503 rather than queued, so many
  concurrent near-cap bodies cannot OOM the worker (upstream #330).
- **A nested `choice` label is a named caller error.** A list-of-labels whose entry is itself an
  array or object is rejected naming the question and the index, instead of being silently rendered
  as its JSON text (and, over HTTP, becoming an opaque 500). A label is the answer key and option
  text, so it must be a scalar (upstream #425).

### Notes on upstream changes not needing a Rust change
- **Tokenizer thread-safety (#... "serialise fast-tokenizer encoding").** The upstream lock guards
  a shared Python fast tokenizer whose `truncation=True`/`padding=True` mutate it. The Rust
  tokenizer's `encode` takes `&self` and never mutates; option truncation is done by slicing token
  ids, so concurrent `predict()` calls are already safe. No-op here.
- **Meta-device head init (#... "build the decision head without initialising overwritten
  weights").** A torch-only speedup to skip weight initialisation the checkpoint overwrites. The
  Rust port loads every parameter from `safetensors` directly; there is no random init to skip.
- **CUDA `LAYA_CUDA_AMP`, XPU autocast/eviction, scoped CUDA-OOM CPU fallback.** All torch device
  backends the Rust port does not have (it targets fp32 on candle's CPU backend; Metal is a
  separate opt-in). No counterpart.
- **Hooks `hooks_timeout` / run-default-hooks-once / TileLang fast-path dtype.** The Rust `Router`
  and `Agent` have neither process-wide hooks nor the TileLang fast path, as noted in 0.2.1.
- **`structured.py`, `evals`, `mcp`, benchmarks, docs, and the `laya-ts` parity fixes.** Not part
  of the ported surface (structured output, the evaluation harness, the MCP server and the
  TypeScript reference are all out of scope per `PLAN.md`); the `laya-ts` fixes bring that port in
  line with the Python behaviour the Rust golden fixtures already track.

### Deferred (upstream features not yet ported)
- **Hub revision pinning + opt-in SHA-256 verification** (`revision`/`revisions`/`expected_sha256`)
  and **`Agent.predict_long`** (windowed scan of over-long states) and the shortlist
  **`cached_embed_fn`** are additive features that need model weights / the download path to test
  meaningfully; they are left for a follow-up rather than shipped unverified in a patch release.

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
