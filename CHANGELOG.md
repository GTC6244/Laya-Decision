# Changelog

All notable changes to Laya-Decision are documented here. This project ports the upstream
[Laya](https://github.com/NandhaKishorM/laya) engine; each entry notes the upstream version tracked.

## 0.2.13 — tracks upstream Laya 0.4.11 (upstream @d74e8be)

Reviews the upstream `laya/` and `laya-ts/` changes made after the previous sync (upstream
@ca5045a, 0.4.9) across the 0.4.10 and 0.4.11 releases. The 92-commit span is dominated by a brand
new, independent upstream `laya-rust/` SDK (a separate crate — not this port, which upstream does
not vendor or depend on), the TypeScript SDK catching up to Python with `decideBatch` /
`answerConfidence`, two new integration wrappers (Agno team routing and a `LayaToolSelector`), the
ONNX-export test/tooling overhaul, and modules this port does not carry (`calibrate`, `evals`,
`hooks`, `mcp`, the structured `decide`/`decide_batch` schema layer). **No change in this span
reaches the carried Rust surface, so no port lands and the golden parity fixtures are untouched.**
This is a tracking release: it refreshes the vendored upstream snapshot to 0.4.11 and bumps
`UPSTREAM_VERSION`, keeping the published crate aligned with the upstream release it mirrors.

### Unported upstream changes

The carried-adjacent `laya/` and `laya-ts/` files that moved all land on surface this port does not
carry:

- **`calibrate.fit_temperature_map` now reports a `temperature_clamped` map** naming every fit the
  `[TEMP_MIN, TEMP_MAX]` clamp moved, carrying the unclamped optimum beside the installed value
  (upstream `calibrate.py`, "fix(calibrate): report a temperature fit the clamp moved"). This is the
  calibration *fitter* — an LBFGS/NLL training-time routine. This port carries only calibration
  *application* (the `clamp_temperature` guard at checkpoint load); it has no temperature fitter, so
  there is no landing site, and `clamp_temperature`'s own behaviour is unchanged.
- **`onnx_agent` collates encoded rows directly into NumPy arrays** via a new `_collate_items_numpy`
  helper, replacing the old `collate_items` (torch) + `.numpy()` conversion path (upstream
  `onnx_agent.py`, "perf(onnx): collate question rows directly into numpy arrays"). It is a
  performance refactor producing byte-identical arrays — same pad id, same attention mask, same
  floor of ≥2 marker columns that the exported `topk(2)` head needs — and this port's Rust
  `collate_items` already pads to that floor. The same change adds `model_id_or_path` / `subfolder`
  attributes so the v2 calibration-payload identity check can match; that check backs the uncarried
  calibration-fingerprint surface.
- **`hooks.validate_timeout` now refuses a bool `hooks_timeout`** rather than reading `True` as
  `1.0` (upstream `hooks.py`). This port carries no hooks surface, so there is no code site.
- **The TypeScript SDK gained `Agent.decideBatch` / `Router.decideBatch` and a
  `DecisionResult.answerConfidence` field** (upstream `laya-ts/src/{agent,router,structured,index}.ts`
  with new coverage in `tests/{agent,router-batch,structured}.test.ts`). These port the Python
  `decide_batch` schema API and expose the per-field `answer_confidence` through the structured
  layer. This port carries no `structured` module (the `decide`/`decide_batch` schema API is not
  carried) and no `predictBatch`/`routeBatch` grouping path; the underlying `answer_confidence` is
  already carried as `laya::answer_confidence`, so nothing new lands.
- **The integration wrappers moved**: `integrations/_controls` and the LangChain / CrewAI /
  LlamaIndex nodes now forward the two Router-only routing hints (`task`, `lang_guess`), and two new
  wrappers arrived — `integrations/agno.py` (Agno multi-agent team routing) and
  `integrations/tools.py` (`LayaToolSelector` for sub-35 ms tool/function routing). These are
  user-facing integration wrappers with no Rust analogue.
- **`mcp/server` and `mcp/tools` added a `laya_predict_long` tool** (and gated `task`/`lang` on the
  predict-long path) for states past the context window. MCP is not carried.

Beyond those, the span is confined to the uncarried `evals` module, the independent upstream
`laya-rust/` SDK, the ONNX-export scripts/tests, the `laya-go` / `laya-java` / `laya-dotnet`
bindings, example launchers, docs, CI and packaging.

The golden parity fixtures are **byte-for-byte identical** to 0.2.12: language detection, routing,
email cleaning, confidence and the served surface did not move between 0.4.9 and 0.4.11. Vendored
`reference/` snapshots synced to upstream @d74e8be — the new `integrations/agno.py` and
`integrations/tools.py` are vendored alongside the other integration wrappers, and the TypeScript
`src`/`tests` snapshots are refreshed. `UPSTREAM_VERSION` now reads `0.4.11`.

## 0.2.12 — tracks upstream Laya 0.4.9 (upstream @ca5045a)

Reviews the upstream `laya/` changes made after the previous sync (upstream @4ac226d, 0.4.8) across
the single 0.4.9 release. The 85-commit span is again dominated by the `laya-go` binding (hooks,
the confidence gate, per-language temperatures and checkpoint-digest verification) plus Java/.NET,
CI, docs and packaging work, and by modules this port does not carry (`calibrate`, `evals`,
`evals_cli`, `evidence`, `mcp`, the `_agreement` eval-fingerprint helper and the `backends/compile`
torch-inductor cache). **No change in this span reaches the carried Rust surface, so no port lands
and the golden parity fixtures are untouched.** This is a tracking release: it refreshes the
vendored upstream snapshot to 0.4.9 and bumps `UPSTREAM_VERSION`, keeping the published crate aligned
with the upstream release it mirrors.

### Unported upstream changes

The three carried-adjacent `laya/` files that moved all land on surface this port does not carry:

- **`router.predict_batch` now memoizes the question-schema serialization across consecutive
  contexts that share the same `questions` object, and spells the empty-override case as `{}`
  directly** (upstream `router.py`, "perf(router): memoize question schema serialization in
  predict_batch"). Both halves are performance/clarity only — the `overrides` dict-comprehension
  already produced `{}` when both token budgets were unset, and the memoization is a Python
  object-identity cache. This port's `Router` has no batched `predict_batch` grouping path (routing
  resolves one request to one model; token-budget overrides are an agent-level `predict` concern),
  so there is no code site for either edit.
- **`agent._cpu_amp_dtype` gained a docstring paragraph explaining that a host without native BF16
  instructions dies with a native `SIGILL` / `0xC000001D` fault that the autocast `RuntimeError`
  retry cannot catch** (upstream `agent.py`). Documentation only, and specific to torch's CPU
  autocast path; this port runs inference through candle with no autocast retry, so the warning has
  no landing site.
- **`_agreement._json` now matches JSON containers by `isinstance` (so a `dict`/`list` subclass such
  as `OrderedDict` validates as the mapping it encodes to) while keeping exact-type matching for
  scalars** (upstream `_agreement.py`). The `_agreement` module backs the uncarried `evals` /
  `calibrate` fingerprinting surface and is not vendored or ported here.

Beyond those, the span is confined to the uncarried `calibrate`, `evals`, `evals_cli`, `evidence`,
`mcp.device`/`mcp.server` and `backends.compile` modules, and to the `laya-go` / `laya-java` /
`laya-dotnet` / `sdk` bindings, docs, CI and packaging.

The golden parity fixtures are **byte-for-byte identical** to 0.2.11: language detection, routing,
email cleaning, confidence and the served surface did not move between 0.4.8 and 0.4.9. Vendored
`reference/` snapshots synced to upstream @ca5045a, and `UPSTREAM_VERSION` now reads `0.4.9`.

## 0.2.11 — tracks upstream Laya 0.4.8 (upstream @4ac226d)

Reviews the upstream `laya/` changes made after the previous sync (upstream @147a639, 0.4.7) across
the single 0.4.8 release. The 75-commit span is overwhelmingly the new `laya-go` binding (a whole
`laya-go/agent`, `laya-go/onnx` and `laya-go/infer` tree) plus Java/CI/docs/packaging work and
modules this port does not carry (`calibrate`, `evals`, `evals_cli`, `evidence`, `mcp`,
`onnx_agent`, `revisions`). Three changes reach the carried surface and are ported; the rest have no
code site on the Rust surface and are listed below.

### Fixed
- **`jev_confidence` now refuses a negative probability instead of normalising it into range
  (upstream `confidence.py`).** The two Jev formulas normalise by the sum of the probabilities, so a
  mixed-sign document could divide by a near-zero or negative total and return a number outside
  `[0, 1]` — for a distribution whose argmax is not the one the formula measured — while an
  all-negative document divided back into range and returned exactly what its positive counterpart
  would. A malformed document — a probability that does not read as a number, a boolean, a
  non-finite **or now a negative value** — gives `None`, so a caller filtering on
  `x_jev_confidence` never filters on a made-up one and every number this returns lies in `[0, 1]`
  ([`laya::common::jev_confidence`], served through `laya-serve`'s `add_jev_confidence`).
- **`laya-serve` reads a blank `LAYA_PORT` as the documented default instead of exiting (upstream
  `serve._resolve_port`).** `LAYA_PORT=` — what a `.env` line or Compose `KEY=` left for the operator
  to fill in writes — trimmed to the empty string, failed the integer parse and exited `2` with an
  "invalid LAYA_PORT" message. Unset and blank now both bind `8000`; a value that names a number and
  the wrong one (`0`, `99999`, `abc`) still exits loudly, because binding a server somewhere its
  operator did not ask for cannot be contradicted by falling back.
- **`laya-serve` reads a blank `LAYA_*` boolean and a blank `LAYA_LOG_LEVEL` as "not specified"
  (upstream `serve._env_bool` and the new `serve._resolve_log_level`).** A declared-but-unfilled
  `LAYA_PRELOAD=` or `LAYA_JEV_STRICT=` / `LAYA_AUTO_TASK=` trimmed to the empty string, missed the
  four on-spellings and silently read as *off* — now it keeps the switch's documented default. A
  blank `LAYA_LOG_LEVEL=` likewise takes the default `info` rather than installing an empty tracing
  filter. (The upstream helper's uvicorn-`LOG_LEVELS` validation is Python-specific: this port logs
  through `tracing_subscriber`'s `EnvFilter`, which accepts richer directives than uvicorn's five
  level names and already falls back to `info` on an unparseable value, so only the blank-means-
  default half of the change has a landing site here.)

### Unported upstream changes

The rest of the carried-surface churn has no landing site on this port and is deliberately skipped:

- **`agent._encode_state` reading `n_opts` from the encoder's `stats["options"]` instead of
  re-calling `render_options(q)`** (upstream `agent.py`): a performance/cleanliness change with no
  behavioural effect — the two counts are equal. The Rust marker guard
  (`markers.len() != render_options(q)?.len()`) already compares against the true option count, and
  `render_options` is cheap, so there is nothing to correct.
- **`common.ece_score` refusing a `bins` that is not an integer ≥ 1** (upstream `common.py`): Expected
  Calibration Error is part of the uncarried `calibrate`/`evals` surface; the Rust port has no
  `ece_score`.
- **`structured._enum_field` rejecting a non-array `enum` and treating a boolean enum as `noul` only
  when it has exactly two distinct values** (upstream `structured.py`): the `structured` `decide()` /
  `decide_batch()` schema API is not carried by this port (no `structured` module on the Rust
  surface).
- **Docstring-only edits in `email.clean_email_body` / `email.email_state`** (upstream `email.py`,
  the `budget-zero` / `budget-negative` behaviour) — the Rust `clean_email_body` already slices by
  the same budget and is covered by the `email_parity` golden vectors, which are unchanged.
- Changes confined to the uncarried `calibrate`, `evals`, `evals_cli`, `mcp.server`, `onnx_agent`
  and `revisions` modules, and to the `laya-go` / `laya-java` / `sdk/typescript` bindings, docs and
  packaging.

The golden parity fixtures are **byte-for-byte identical** to 0.2.10: language detection, routing
and email cleaning did not move between 0.4.7 and 0.4.8 (`lang`, `email`, `presets`, `router` and
`structured` have no carried behavioural diff, and the `confidence` change only *rejects* an input
the golden vectors never carry). Vendored `reference/` snapshots synced to upstream @4ac226d, and
`UPSTREAM_VERSION` now reads `0.4.8`.

## 0.2.10 — tracks upstream Laya 0.4.7 (upstream @147a639)

Reviews the upstream `laya/` changes made after the previous sync (upstream @682ac67, 0.4.4) across
the 0.4.5, 0.4.6 and 0.4.7 releases. The 159-commit span is mostly language bindings (`laya-java`,
`laya-dotnet`, `laya-ts`, `laya-go`), docs/compose/ci work, and modules this port does not carry
(`calibrate`, `evals`, `evals_tournament`, `evidence`, `mcp`, `onnx_agent`, and the
`Router.predict_batch` / batch-endpoint surface). Two changes reach the carried surface and are
ported; the rest have no code site on the Rust surface and are listed below.

### Changed
- **`laya-serve` narrows an over-cap tournament choice instead of reading it in one pass (upstream
  #1062).** With `LAYA_TOURNAMENT_GROUP` set, the choice-option cap is raised to 255
  (`MAX_TOURNAMENT_CHOICE_OPTIONS`), but the one-pass read behind `MAX_CHOICE_OPTIONS` (100) was
  never sized for a choice that large. Before, a large choice carrying an `option_order` — or an
  array-criteria choice with a non-string label — sent the *whole* request back to the one-pass
  path, which for a choice over 100 options meant reading more options in a single span than that
  path is built for. Now, once any choice in the request is over `MAX_CHOICE_OPTIONS` (a size only
  the raised cap admits and one pass was never asked to read), the request must be answered by
  elimination: such a choice's `option_order` is dropped — it no longer fits the kept labels once a
  round cuts the choice — and the drop is reported as `x_tournament.option_order_dropped: true`,
  while a choice whose labels are not all strings (an eliminated-option probability is keyed by a
  string label) is refused with 422 rather than silently one-passed. A request whose large choices
  are all within `MAX_CHOICE_OPTIONS` keeps the previous behaviour: an `option_order` or a
  non-string label still sends it to the one-pass path. Strict mode (`LAYA_JEV_STRICT`) still serves
  only the contracted keys, so `x_tournament` — and the new marker inside it — is dropped there
  (upstream `serve._tournament_plan`, `feat(serve)`).

### Fixed
- **The `laya` CLI's interactive mode now reports a failed request in its exit code.** `laya` with
  no request text reads one request per line from stdin; a handled failure (a `laya: …` line on
  stderr and exit `2` from `run`) stays non-fatal to the session, but the loop discarded that code
  and the process always exited `0`. `laya < requests.txt` therefore exited `0` with answers missing
  and only stderr to say so. The loop now keeps the last non-zero code and returns it on exit, so a
  pipeline sees the failure (upstream `cli.interactive`).

### Unported upstream changes

The rest of the carried-surface churn has no landing site on this port and is deliberately skipped:

- **`temperature_rejected` and the `resolve_lang_temperatures` warnings** (upstream `common.py`,
  and `agent.py`'s checkpoint-load diagnostic that now routes through it): the helper distinguishes
  a replaced temperature from an accepted one so a bool `true` (which `float(True) == 1.0` had made
  look accepted) is named in a `RuntimeWarning`. The Rust port carries no `lang_temperatures`
  per-language option and emits no temperature-load warning at all — `load_temperature` /
  `load_temperature_by_options` clamp silently, and `clamp_temperature` already routes a JSON bool
  to the neutral `1.0` fallback — so there is no diagnostic to correct.
- **`Router.predict_batch` raising `TypeError` on a `None` `state`** (upstream `router.py`): the
  Rust `Router` has no `predict_batch`.
- **`shortlist._call_predict`'s dict check consolidation and the new `_round_answers`** (upstream
  `shortlist.py`): these name the agent-reply contract (`a dict`, with an `answers` dict) instead of
  raising a bare indexing error. The Rust tournament calls the strongly-typed `agent.system_one`,
  which cannot return a non-dict, and the round loop already guards the `answers` lookup, so the
  checks are no-ops on the typed surface.
- **`shortlist.cached_embed_fn`'s NaN-caching docstring** (upstream `shortlist.py`): the Rust
  shortlist takes a caller-supplied `embed_fn` directly and carries no embedding cache.
- **`serve`'s batch-endpoint changes** — `_check_batch_limits` taking a `max_choice_options`
  argument, and `create_app`'s `_do_batch` running the tournament per state (upstream `serve.py`):
  `laya-serve` exposes no batch endpoint.
- **`load(...) -> Union[Agent, "ONNXAgent"]`'s return annotation** (upstream `agent.py`): the ONNX
  backend is not carried.
- Docstring-only edits in `common.confidence_from_probs` (the `k < 2 → 1.0` behaviour, unchanged and
  mirrored by the Rust `confidence_from_probs`) and elsewhere.

The golden parity fixtures are **byte-for-byte identical** to 0.2.9: language detection, routing,
and email cleaning did not move between 0.4.4 and 0.4.7 (`lang`, `email`, `presets`, `structured`
and `confidence` have zero diff lines, and `router`'s only change is on `predict_batch`, which
`route` never reaches). Vendored `reference/` snapshots synced to upstream @147a639, and
`UPSTREAM_VERSION` now reads `0.4.7`.

## 0.2.9 — tracks upstream Laya 0.4.4 (upstream @682ac67)

Reviews the upstream `laya/` changes made after the previous sync (upstream @6880462, 0.4.2) across
the 0.4.3 and 0.4.4 releases. The 169-commit span is mostly language bindings (`laya-java`,
`laya-dotnet`, `laya-ts`, `laya-go`), docs/compose/ci work, and modules this port does not carry
(`train`, `evals`, `calibrate`, `evidence`, `backends`, `mcp`, `tl_kernels`, `_agreement`). One
change reaches the carried surface and is ported; the rest have no code site on the Rust surface and
are listed below.

### Added
- **`LAYA_TOURNAMENT_GROUP` on `laya-serve`.** `/v1/systemone` can now answer a choice with more
  options than the group size *by elimination*, in groups of that size, rather than reading every
  option in one pass — the point past which options no longer each get a token span of their own
  (upstream #302, `feat(serve)` `c3e631b`, with the one-pass-cap bound from `fix(serve)` `472f098`).
  When the variable is set (an integer `2..=100`; anything else leaves it off), a choice over the
  group runs through the library's `predict_tournament`; each answer keeps the Jev shape —
  `probabilities` names every option in criteria order, the finalists carry the final round's
  probabilities and an eliminated option carries `0.0`, so `choice` is still the argmax and the
  values still sum to 1 — and an `x_tournament` entry records the finalists and the round count.
  `usage.input_tokens` is summed over every forward pass (a new
  `shortlist::predict_tournament_with_usage` returns that total beside the otherwise-unchanged
  result); the other usage keys are the final call's. A choice carrying an `option_order`, or an
  array-criteria choice with a non-string label, keeps the one-pass path, because a narrowed choice
  cannot carry an order over the full label set and an eliminated-option probability is keyed by a
  string label (upstream #1062). Setting the variable also raises the HTTP choice-option cap from
  100 to 255 (`MAX_TOURNAMENT_CHOICE_OPTIONS`), since every choice the raised cap newly admits is
  always over the group and so always narrowed, never handed to a one-pass read that cannot fit it.
  Strict mode (`LAYA_JEV_STRICT`) still serves only the contracted keys, so `x_tournament` is
  dropped there.

### Unported upstream changes

The rest of the carried-surface churn has no landing site on this port and is deliberately skipped:

- **`flag_low_confidence` / `apply_confidence_gate` clearing a stale marker at `min_confidence`
  `0.0`** (upstream #910, `fix(confidence)` `2863500`): the Rust single-request surface carries no
  `min_confidence` gate (as `laya-serve`'s predict path already notes), so there is no marker to
  clear and no loop to run.
- **`Router.predict_batch`'s `diagnostics` out-parameter and the CLI/serve plumbing around it**
  (upstream #1111, `a69fa97` / `9f7a6a6`): the Rust `Router` has no `predict_batch`, the CLI has no
  batch mode, and `laya-serve` exposes no batch endpoint, so the per-group `sort_by_length` report
  and its HTTP refusal (`BATCH_BODY_OUT_PARAMETERS`) reach no code.
- **`cli.load_questions` disambiguating a bare `questions` question id from the wrapper object**
  (upstream `6e5ad07`): the Rust CLI answers presets, not a `--questions` JSON file.
- **`shortlist.cached_embed_fn` taking the cache lock around its dimensionality guard** (upstream
  `586366e`): the Rust shortlist takes a caller-supplied `embed_fn` directly and carries no
  embedding cache to guard.
- Docstring-only moves in `lang.analyse` (`mixed_segment`), `structured.decide_batch`, and `serve`'s
  `LAYA_DEFAULT_MODEL` column (upstream's `Router` has defaulted to `multilingual` since 0.4.0; this
  port's `Router` still defaults to `english`, so the server's env table is already right and is
  left unchanged).

The golden parity fixtures are **byte-for-byte identical** to 0.2.8: language detection, routing,
and email cleaning did not move between 0.4.2 and 0.4.4 (`lang`'s change is a docstring, `router`'s
is on `predict_batch`, which `route` never reaches, and `email` did not change at all). Vendored
`reference/` snapshots synced to upstream @682ac67, and `UPSTREAM_VERSION` now reads `0.4.4`.

## 0.2.8 — tracks upstream Laya 0.4.2 (upstream @6880462)

Reviews the upstream `laya/` changes made after the previous sync (upstream @1adc59f, 0.4.1) up to
the 0.4.2 release. The 39-commit span is mostly the language bindings (`laya-java`, `laya-dotnet`,
`laya-ts`), `compose` / `evals` / `docs` / `ci` work, and modules this port does not carry (`train`,
`evals`, `_agreement`, `calibrate`, `evidence`, `backends`). Two changes reach the carried surface
and are ported; one is a registration-time diagnostic with no analogue on the Rust surface and is
deliberately skipped (detailed below).

### Added
- **`x_jev_confidence` on the server's choice and score answers.** In the non-strict wire path,
  `laya-serve` now reports, beside Laya's own `confidence`, the number Jev's own formula gives for
  the same probabilities — `(max(p) - 1/n) / (1 - 1/n)` for a choice, and a mode-spread ratio
  `max(0, 1 - Σ pᵢ·|i − mode| / D)` for a score — so a client porting a threshold from Jev reads a
  comparable value rather than Laya's normalized entropy, which selects about twice as much at the
  same cutoff. The `x_` prefix keeps it clear of any field the Jev contract may add later; a noul
  answer, or probabilities that give no usable number (missing, empty, non-numeric, boolean,
  non-finite, or a non-integer score level), carries no field rather than a guess; and strict mode
  (`LAYA_JEV_STRICT`) still serves only the contracted keys. The formula lives in the library as
  `common::jev_confidence` (upstream #302, `feat(serve)` `c932543`; upstream
  `laya/confidence.py::jev_confidence`).

### Fixed
- **A shortlisted or tournament-narrowed choice no longer carries a stale `option_order`.** An
  `option_order` holds one slot per option, so an order sized for the caller's full label set no
  longer fits once `predict_shortlist` or `predict_tournament` cuts the criteria, and the agent
  rejected the question outright (`'option_order' must be a permutation of range(k)`). The narrowed
  copy now drops the order — with a one-shot stderr notice naming the question — because remapping it
  onto the kept labels would silently move options between slots. A passthrough choice (nothing was
  dropped) and non-choice questions keep their order, and the caller's own question dict is never
  mutated (upstream #958, `fix(shortlist)` `f707d29`).

The one behavioural change left unported is the **router's "name is one edit from a built-in"
warning** (upstream `feat(router)` `d728d6f`): it fires only when *registering a custom checkpoint
name*, and the Rust `Router` has no register/attach path — its model map is fixed from the built-in
defaults (or the standalone repos) at construction — so there is no code site the edit-distance
check could guard. Its sibling `test(router): document and verify explicit port defaults` is a
tests-only change over behaviour the port already matches.

The golden parity fixtures were regenerated from upstream 0.4.2 and are **byte-for-byte identical**
to 0.2.7: language detection, routing, and email cleaning did not move between 0.4.1 and 0.4.2 (the
router change is a registration-time diagnostic that `Router.route` never reaches). Vendored
`reference/` snapshots synced to upstream @6880462, and `UPSTREAM_VERSION` now reads `0.4.2` (it had
drifted from the tracked release).

## 0.2.7 — tracks upstream Laya 0.4.1 (upstream @1adc59f)

Reviews the upstream `laya/` changes made after the previous sync (upstream @e08843b, 0.3.29)
across the 0.4.0 and 0.4.1 releases. **Nothing in this range reaches the Rust pure-logic surface
as a portable change.** The whole 166-commit span is documentation, tooling, language bindings
(`laya-ts`, `laya-java`, `laya-dotnet`), or lives in modules this port does not carry (`train`,
`evals`, `calibrate`, `evidence`, `hooks`, `mcp`, `onnx_agent`, `tl_kernels`, `backends`, `serve`).
Of the three carried Python modules that changed, two are non-behavioural — a `clamp_temperature`
docstring expansion in `common.py`, and a `**kwargs` plumbing refactor in `structured.decide_batch`
(`predict_batch` routing pins) that has no analogue on the Rust surface, where `Agent::predict_batch`
is checkpoint-level and takes typed parameters, not keyword overrides.

The one behavioural change — the `feat(router)!` flip of the stock router default from `english` to
`multilingual`, shipped in upstream 0.4.0 — is **deliberately not ported**, exactly as flagged in the
0.2.6 notes when it was still unreleased. It is a breaking change (upstream marks it `!`), so adopting
it is out of scope for a patch release; the router default stays `english`. Detected non-Latin script
still routes to `multilingual` regardless of the default, so only language-undecided, letterless, or
no-non-English-letter text is affected. A deployment that wants the upstream behaviour constructs
the router with `RouterOptions { default: "multilingual".into(), ..Default::default() }`.

The golden parity fixtures were regenerated from upstream 0.4.1 and are **byte-for-byte identical** to
0.2.6: `lang` and `email` because those modules are unchanged, and `router` because `dump_golden.py`
now pins `Router(default="english")` to match the port across the upstream default flip. This
confirms the pure-logic surface (language detection, routing, email cleaning) did not move between
0.3.29 and 0.4.1. Vendored `reference/` snapshots synced to upstream @1adc59f.

### Changed
- `scripts/dump_golden.py` pins `Router(default="english")` when dumping the router fixture, so the
  fixture tracks the port's retained `english` default rather than upstream's new `multilingual` one.

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
