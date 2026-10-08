# Changelog

All notable changes to Laya-Decision are documented here. This project ports the upstream
[Laya](https://github.com/NandhaKishorM/laya) engine; each entry notes the upstream version tracked.

## 0.2.6 — tracks upstream Laya 0.3.29 (upstream @e08843b)

Ports the upstream `laya/` changes made after the previous sync (upstream @a4a8921, 0.3.28) that
affect the Rust surface, up to the 0.3.29 release. Only the language-detection change reaches the
Rust surface; the rest of the 0.3.28→0.3.29 range is documentation, or lives in modules this port
does not carry (`train`, `evals`, `calibrate`, `evidence`, `hooks`, `mcp`, `onnx_agent`,
`tl_kernels`). The unreleased `feat(router)!` default flip to the multilingual checkpoint, made on
`main` after the 0.3.29 tag and slated for upstream 0.4.0, is deliberately **not** ported: the
router default stays `english`, as upstream 0.3.29 ships it. The golden parity fixtures were
regenerated from upstream 0.3.29 and gained all-caps / shouted cases that exercise the new
behaviour; they are byte-for-byte identical to upstream on every case. Vendored `reference/`
snapshots synced to upstream @e08843b.

### Changed
- **All-caps lines are no longer read as foreign prose.** A line of nothing but acronyms and place
  names — `MON DES EST LA`, `STORES LOS ANGELES LAS VEGAS EL PASO CLOSED` — has no lowercase, so it
  skipped the acronym-blanking that mixed-case prose gets and was named French or Spanish outright.
  A *shouted* segment (cased text written entirely in capitals) must now clear a stronger evidence
  bar before any language is believed: at least one word-length token (≥5 letters) and then either a
  non-English diacritic or a matched stopword long enough (≥4 letters) to not be an acronym. The bar
  is applied in three places that previously disagreed — the single-line prose scan
  (`named_prose_language`), the whole-state verdict (`analyse_text`), and the per-field scan
  (`leaf_non_english`) — so a state that is *nothing but* an acronym line, or diluted by too few
  English lines to tip the verdict, is vetoed just as a hidden line is. A shouted line carried by
  non-English letters (`WIE SPÄT IST ES IN KÖLN`) is still kept foreign; its ASCII spelling
  (`WIE SPAET IST ES IN KOELN`) has no diacritic and only a three-letter stopword, so it routes
  English — a documented upper-cased casualty, not a regression. In mixed-case text, acronym-shaped
  all-caps runs are blanked before the verdict so they do not vote, but emphasis capitals on
  word-length runs (`sag mir das HEUTIGE DATUM`, `quiero cancelar mi PEDIDO POR FAVOR`) are left
  alone, so a shouted word never erases its own sentence (upstream `fix/lang-allcaps-acronyms`,
  commits `9e8c196`, `2ac85c0`).

## 0.2.5 — tracks upstream Laya 0.3.28 (upstream @a4a8921)

Ports the upstream `laya/` changes made after the previous sync (upstream @6d942c9, 0.3.22) that
affect the Rust surface, up to the 0.3.28 release. The golden parity fixtures were regenerated from
upstream 0.3.28 and gained Swedish and French cases that exercise the new behaviour; they are
byte-for-byte identical to upstream on every case. Vendored `reference/` snapshots synced to upstream
@a4a8921.

### Added
- **Swedish language detection.** A Swedish stop list joins the Latin-script guess, plus a short
  two/three-word fragment list that names Swedish below the four-token minimum (`glömt mitt
  lösenord`), a login-phrase branch that reads a distinctive Swedish phrase carrying one
  English-shaped token (`kan inte logga in`), and a Swedish/Danish overlap set that leaves an
  undecided Nordic line on the multilingual checkpoint rather than the English one. ASCII-normalised
  spellings sit beside the diacritic forms so Swedish survives a ticket pipeline that strips its
  letters (upstream `feat(lang)` Swedish).
- **French mail-client cleaning.** `clean_email_body` now cuts French quoted history, signatures,
  device footers and confidentiality disclaimers alongside its EN/PT/ES peers: Gmail's `Le … a
  écrit :`, Outlook's `-----Message d'origine-----` and spaced `De :` / `Envoyé :` reply headers,
  `Cordialement` / `Merci` sign-offs, `Envoyé depuis mon iPhone`, and the `ce message … confidentiel`
  footer (upstream `feat(email)` French / French device-footer spacing).
- **`shortlist_choice_scored` and `predict_tournament`.** `shortlist_choice_scored` returns the
  signed cosine of each kept label beside the labels, matching `shortlist_choice(...,
  return_scores=True)`. `predict_tournament` narrows each large choice question by elimination — one
  shared forward pass per round, no embedder — then answers the full request with each contested
  choice cut to its finalists, adding a `tournament` report (upstream `feat(shortlist)`).
- **`Agent::load` resolves checkpoint names and aliases.** A bare registry name or alias
  (`typed-decisions`, `ml`) now resolves to the same `(repo, subfolder)` the `Router` picks instead
  of being handed to the Hub as a repo id; a path-shaped argument or a directory that holds a
  checkpoint still loads from disk (upstream `load` name resolution + `_is_local_checkpoint_arg`;
  exposed as `router::resolve_model_spec`).
- **CLI `--lang-guess`.** A soft language hint used only when the built-in detector is undecided,
  unlike `--lang` which overrides it — the single-request CLI counterpart of the body control
  `laya-serve` already forwards (upstream `feat(cli)` soft language hint).
- **`laya-serve` honours `LAYA_DEFAULT_MODEL` and `LAYA_JEV_STRICT`.** `LAYA_DEFAULT_MODEL` sets the
  routing fallback for the two states carrying no language evidence (a typo is fatal, not silently
  ignored). `LAYA_JEV_STRICT` projects the response onto the strict Jev wire contract — dropping the
  root `routing` block, each answer's `action` and `answer_confidence`, the `confidence` on `noul`
  answers, and the extended `usage` — for a client that rejects fields the contract does not define
  (upstream `feat(serve)` `LAYA_DEFAULT_MODEL` / `LAYA_JEV_STRICT`).

### Fixed (serve)
- **`laya-serve` rejects a null score level with 422.** A `score` question whose level list holds a
  `null` is a hole in the rubric — the answer's legend would carry `{"<i>": null}`, which a Jev
  client cannot parse — so it is refused at request time naming the question and index, instead of a
  200 with an unparseable legend (upstream #302).
- **`GET /health` no longer leaks deployment internals without the bearer.** On a server locked down
  with `LAYA_API_KEY`, `/health` now answers liveness only (`{"status":"ok"}`, still 200) unless the
  request carries a valid bearer; the resident checkpoint names and device are behind the key, as
  `POST /v1/systemone` already was. With no key configured the full payload stays open (upstream
  #812).
- **`laya-serve` refuses unpublished path-like model ids with 422.** A `model` that names a
  filesystem path or an unpublished Hub `org/repo` id (anything starting with `.`/`~` or containing a
  slash, other than the auto-route bundle id) is refused rather than silently auto-routed to a
  checkpoint the caller did not ask for; a bare Jev id such as `jev-1` still auto-routes (upstream
  #919).

### Notes on upstream changes not needing (or not applicable to) a Rust change
- **Parallel option layout** (`option_layout: "parallel"`, PCW attention) needs transformers≥5 and a
  checkpoint trained for it; every published checkpoint stays sequential and its path is unchanged,
  so porting it changes nothing for supported models.
- **The `laya-train` CLI and dataset loaders**, the **per-option-count abstention thresholds**,
  **abstention-gate state reporting** (`apply_confidence_gate`), the **`predict_long` window budget
  and scan**, **`predict_batch` per-call controls / split passes / usage merge**, the **idle-unload
  worker**, the **checkpoint registry** (`Router(models=…)`), **revision/digest pinning** (and its
  case-folding fix), **`structured` `$defs` resolution**, the **Metal/CUDA cache release**,
  **torch.compile / AOTInductor / autocast / backend layer**, and **ONNX / MCP** changes all live on
  torch/Python/batch/hooks/calibration surfaces the Rust port does not have: it is native candle,
  single-request, with no hooks, no batch path, no calibration plumbing, and no train/ONNX/MCP.
- **`check_min_confidence`/`clamp_temperature` rejecting a bool**, the **CLI UTF-8 output fixes**,
  and the **`agent` reject-non-scalar-choice-label allow-list** are already the Rust port's
  behaviour: its types are statically typed (a temperature is `f64`, a choice label is one of
  `serde_json`'s six `Value` variants), and it writes UTF-8 natively.

## 0.2.4 — tracks upstream Laya 0.3.22 (upstream @6d942c9)

Ports the upstream `laya/` changes made after the previous sync (upstream @9d95567) that affect the
Rust surface, up to the 0.3.22 release. The golden parity fixtures were regenerated from upstream
0.3.22; they are byte-identical to the previous set, because the ported behaviour changes do not
alter any sampled case (the collision-word and device-footer changes only move edge inputs, and the
legend change only affects non-string score levels). Vendored `reference/` snapshots synced to
upstream @6d942c9.

### Changed
- **Email device footers absorb `mobile` and drop the standalone marker.** `mobile` now rides in
  the device alternation, and the whole-line footer also accepts a trailing
  `phone`/`device`/`pro`/`max`/`mini`/`plus` word or a `using <app>` clause. The separate,
  start-anchored `sent from my (iphone|android|mobile|ipad)` signature marker is gone: it cut a
  short line that merely *opened* with a device mention along with the real request after it
  (`Sent from my iPhone, help me`), which is now preserved, while a line that is nothing but a
  footer (`Sent from my mobile`) is still cut (upstream drop-device-marker / extend-footer).
- **Language detection counts a collision word once.** The eight function words that are also
  ordinary English — `come`, `son`, `do`, `care`, `todo`, `im`, `per`, `plus` — count once however
  often they repeat, so a repeated English word (`do more, do less`) no longer scores a foreign
  language high enough to route plain English to the multilingual checkpoint. Every other word still
  counts each occurrence, so `der` twice stays German (upstream `_EN_COLLISION_WORDS`).
- **Score-answer legends render the level text.** A legend value is now `render_criterion(level)` —
  the same JSON text the model was shown — rather than the raw JSON value, so a numeric scale passed
  as `[1, 2, 3]` comes back with string legend values (`{"0": "1", ...}`) instead of the JSON types
  the caller happened to pass, matching `probabilities`, which stringifies its keys (upstream
  render_criterion legend).

### Added
- **Questions may carry `option_order`.** A permutation of the option indices, one slot per option,
  choosing the order the options are shown to the model; the returned probabilities are unpermuted
  back to the caller's option order so the answer is unchanged. A non-permutation is rejected naming
  the question (upstream `option_order` + `unpermute_probs`).
- **`instructions` is validated.** A `null` `instructions` (it would serialize as the text `null`)
  and an empty string/list/dict (nothing for the model to answer) are rejected; a number or bool is
  left alone, the same silent-shape class as the null/duplicate-label checks (upstream instructions
  validation).
- **`laya-serve` forwards the routing controls a JSON body can state.** `task` (an unknown one is a
  422), and `lang` / `lang_guess` (each must be a language-code string; a bool or number is a 422,
  since routing would stringify it into a real code). Each is sent only when the client sent it, so
  an absent field still inherits what the `Router` was built with (upstream `BODY_CONTROLS`).

### Changed (serve)
- **`laya-serve` measures the state limit on the tokenizer's text.** `MAX_STATE_CHARS` is now checked
  on `serialize_state(state)` — the string itself, or `", "`/`": "`-separated JSON for a dict/list —
  not `to_string()`'s compact form, so the cap counts the same characters the model is charged for
  (upstream measure-state-limit-on-tokenizer-text).

### Notes on upstream changes not needing (or not applicable to) a Rust change
- **Truncation stats in `usage`** (#174), the **`predict_long` usage merge**, **`predict_batch`
  `sort_by_length` / per-request token budgets**, the **opt-in calibration file**
  (`fit_temperatures` / `save_calibration` / `load_calibration`), **`warmup()`**, the **CUDA
  autocast weight cache** and the **MPS/CPU AMP-failure streak** (#351) all live on torch/Python
  surfaces the Rust port does not have: it is native candle, single-request, with no hooks, no batch
  path, and no calibration plumbing.
- **`lang_temperatures` shape validation** (`resolve_lang_temperatures`) does not apply: the Rust
  `Agent` does not accept per-language temperature overrides.
- **`Router.predict` / `route` `TypeError` guards** for a `None` or non-dict `state`/`questions` are
  enforced by the Rust type system.
- **CLI `--sort-by-length`** is a `--batch`-mode flag; the Rust CLI is single-request.
- **`laya-serve` `/v1/systemone/batch`, `LAYA_ROOT_PATH`, `_refuse_body_refusals` (hooks), the
  lone-surrogate guard and `min_confidence`** do not apply: the Rust server is single-request,
  generates no OpenAPI, has no hooks or abstention gate, and serde parses only valid UTF-8, so a lone
  surrogate cannot survive into a `Value`. The published-model-id lookup and the `_KNOWN_MODELS` drop
  are behaviour-preserving refactors — `normalise_name` already returns only the routable names.
- **The ONNX / TileLang / `compile=` / finetune / evals / MCP / integrations / TypeScript-SDK work**
  has no Rust counterpart.

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
