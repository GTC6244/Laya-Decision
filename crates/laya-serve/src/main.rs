//! Jev-compatible HTTP server for the Laya-Decision engine. Ported from `laya/serve.py`.
//!
//! Exposes `POST /v1/systemone` (the TypeSafe Jev wire protocol) and `GET /health`. Inference
//! is CPU-bound and runs on a blocking worker behind a single-permit gate so one request never
//! stalls the event loop. Configuration is entirely via environment variables:
//!
//! | env var          | meaning                                              | default |
//! |------------------|------------------------------------------------------|---------|
//! | `LAYA_HOST`      | bind address                                         | 0.0.0.0 |
//! | `LAYA_PORT`      | bind port                                            | 8000    |
//! | `LAYA_DEVICE`    | device for every checkpoint                          | (cpu)   |
//! | `LAYA_PRELOAD`   | build checkpoints at startup, not lazily             | 1       |
//! | `LAYA_MODELS`    | comma list to preload (english,multilingual,typed-decisions) | (all) |
//! | `LAYA_AUTO_TASK` | auto-route to the typed-decisions checkpoint         | 0       |
//! | `LAYA_DEFAULT_MODEL` | routing fallback for states with no language evidence | english |
//! | `LAYA_API_KEY`   | if set, require `Authorization: Bearer <it>`         | (none)  |
//! | `LAYA_JEV_STRICT`| project responses onto the strict Jev wire contract  | 0       |
//! | `LAYA_MAX_CONCURRENT` | in-flight requests admitted before shedding 503 | 16      |
//! | `LAYA_TOURNAMENT_GROUP` | answer a choice with more options than this by elimination in groups of this size; raises the choice-option cap from 100 to 255 (#302) | 0 (off) |

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router as AxumRouter,
};
use laya::error::LayaError;
use laya::router::{normalise_name, RouteHints, Router, RouterOptions, BUNDLE_REPO};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

/// Checkpoint names the router understands.
const KNOWN_MODELS: [&str; 3] = ["english", "multilingual", "typed-decisions"];

// Guardrails for unauthenticated remote input. The state is tokenized once per question and
// collated into one tensor, so an unbounded body can OOM the worker; the single-permit gate means
// one large request would also starve /health. Mirrors the limits in upstream `laya/serve.py`.
const MAX_QUESTIONS: usize = 64;
const MAX_STATE_CHARS: usize = 50_000;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
// HTTP-only amplification guard; the library keeps its own head_max_len-aware budget. A choice or
// score question collates one tensor row per option, so an unbounded option count on an otherwise
// small request OOMs the worker just as a huge state would (upstream #335).
const MAX_CHOICE_OPTIONS: usize = 100;
// With `LAYA_TOURNAMENT_GROUP` set, a choice is narrowed in groups rather than read in one pass, so
// its cap is Jev's documented one (255) instead (#302). The tournament reads every label within a
// group's option budget, so the one-pass budget behind `MAX_CHOICE_OPTIONS` no longer binds. The
// resolved group is clamped to `MAX_CHOICE_OPTIONS`, so every choice this cap newly admits has more
// options than the group and is always narrowed, never handed to a one-pass read that cannot fit it.
const MAX_TOURNAMENT_CHOICE_OPTIONS: usize = 255;
const MAX_SCORE_LEVELS: usize = 32;
const MAX_TOTAL_OPTIONS: usize = 512;
// Concurrent requests admitted before the single inference gate. Many near-cap bodies buffered
// while waiting for the one worker can OOM the process even though each request is valid, so
// admission is bounded with a non-blocking check and the excess gets 503 (upstream #330).
const DEFAULT_MAX_CONCURRENT: usize = 16;

/// Constant-time byte comparison, so bearer-token checks do not leak the token by timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Clone)]
struct AppState {
    router: Arc<Router>,
    gate: Arc<Semaphore>,
    /// Non-blocking admission bound on in-flight requests, held from just after auth through the
    /// response so many buffered bodies cannot pile up behind the single inference gate (#330).
    admission: Arc<Semaphore>,
    api_key: Option<String>,
    device: String,
    /// `LAYA_JEV_STRICT`: project the response onto the strict Jev wire contract.
    jev_strict: bool,
    /// `LAYA_TOURNAMENT_GROUP`: group size for choices answered by elimination; 0 is off (#302).
    tournament_group: usize,
}

/// Project a result onto the strict Jev wire contract (`LAYA_JEV_STRICT`).
///
/// The Jev `/v1/systemone` response defines exactly three top-level fields (`model`, `answers`,
/// `usage`), and each answer carries only its type's fields: choice = `choice` + `probabilities` +
/// `confidence`, score = those + `legend`, noul = `noul` only. Laya's full payload adds a root
/// `routing` report, a per-answer `action` head, the calibrated `answer_confidence`, and a
/// `confidence` on noul answers — the fields a client validating against the contract with no extra
/// fields may reject. This keeps only the contracted keys; nothing is recomputed, and an answer of
/// an unknown shape passes through unchanged (upstream `_project_jev_strict`).
fn project_jev_strict(result: Value) -> Value {
    let Some(obj) = result.as_object() else {
        return result;
    };
    let mut answers = serde_json::Map::new();
    if let Some(Value::Object(ans)) = obj.get("answers") {
        for (qid, answer) in ans {
            let get = |k: &str| answer.get(k).cloned().unwrap_or(Value::Null);
            let projected = match answer.get("type").and_then(|v| v.as_str()) {
                Some("choice") => json!({
                    "type": "choice",
                    "choice": get("choice"),
                    "confidence": get("confidence"),
                    "probabilities": get("probabilities"),
                }),
                Some("score") => json!({
                    "type": "score",
                    "score": get("score"),
                    "confidence": get("confidence"),
                    "probabilities": get("probabilities"),
                    "legend": get("legend"),
                }),
                Some("noul") => json!({ "type": "noul", "noul": get("noul") }),
                _ => answer.clone(),
            };
            answers.insert(qid.clone(), projected);
        }
    }
    let usage = obj.get("usage").and_then(|v| v.as_object());
    let token = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .cloned()
            .unwrap_or_else(|| json!(0))
    };
    json!({
        "model": obj.get("model").cloned().unwrap_or(Value::Null),
        "answers": Value::Object(answers),
        "usage": {"input_tokens": token("input_tokens"), "output_tokens": token("output_tokens")},
    })
}

/// Add `x_jev_confidence` to every `choice` and `score` answer: the `confidence` Jev's own formula
/// gives for the same probabilities (upstream #302, served in the non-strict path only).
///
/// Laya's `confidence` is normalized entropy and keeps that meaning; a client porting a threshold
/// from Jev reads `x_jev_confidence` instead. The `x_` prefix keeps it clear of any field Jev may
/// add later. An answer whose probabilities give no usable number is left without the field rather
/// than given a guess, and the result is rebuilt rather than mutated in place. Mirrors upstream
/// `serve._add_jev_confidence`, using the library's [`laya::common::jev_confidence`].
fn add_jev_confidence(result: Value) -> Value {
    let Some(obj) = result.as_object() else {
        return result;
    };
    let Some(Value::Object(ans)) = obj.get("answers") else {
        return result;
    };
    let mut answers = serde_json::Map::with_capacity(ans.len());
    for (qid, answer) in ans {
        let mut projected = answer.clone();
        if let Some(conf) = laya::common::jev_confidence(answer) {
            if let Some(map) = projected.as_object_mut() {
                map.insert(
                    "x_jev_confidence".to_string(),
                    json!(laya::common::round4(conf)),
                );
            }
        }
        answers.insert(qid.clone(), projected);
    }
    let mut out = obj.clone();
    out.insert("answers".to_string(), Value::Object(answers));
    Value::Object(out)
}

/// Decide whether a request takes the tournament (elimination) path, and if so name each large
/// choice's full label set in criteria order (upstream `serve._predict_with_tournament`'s `large`).
///
/// Returns `Some(large)` only when at least one choice has more than `group` options and *every*
/// such choice can be narrowed: a large choice carrying an `option_order`, or with a non-string
/// label, sends the whole request to the one-pass path instead (`None`), because narrowing cannot
/// carry an order over the full label set and an eliminated-option probability is keyed by a string
/// label (#1062). A non-choice question, or a choice within `group`, is left out of the map.
fn tournament_targets(
    questions: &laya::Questions,
    group: usize,
) -> Option<Vec<(String, Vec<String>)>> {
    let mut large: Vec<(String, Vec<String>)> = Vec::new();
    for (qid, question) in questions {
        let Some(q) = question.as_object() else {
            continue;
        };
        if q.get("type").and_then(|v| v.as_str()) != Some("choice") {
            continue;
        }
        let labels: Vec<String> = match q.get("criteria") {
            Some(Value::Object(m)) if m.len() > group => m.keys().cloned().collect(),
            Some(Value::Array(a)) if a.len() > group => {
                let mut labels = Vec::with_capacity(a.len());
                for el in a {
                    // A non-string label cannot key an eliminated-option probability; the whole
                    // request falls back to the one-pass path, as upstream's `all(isinstance str)`.
                    labels.push(el.as_str()?.to_string());
                }
                labels
            }
            _ => continue,
        };
        // An `option_order` is sized for the full label set and no longer fits once a round cuts
        // the choice; a large choice carrying one sends the request one-pass (upstream #1062).
        if q.get("option_order").is_some_and(|v| !v.is_null()) {
            return None;
        }
        large.push((qid.clone(), labels));
    }
    if large.is_empty() {
        None
    } else {
        Some(large)
    }
}

/// Project a [`laya::shortlist::predict_tournament_with_usage`] result onto the Jev wire shape
/// (upstream `serve._predict_with_tournament`'s post-processing).
///
/// For each large choice that actually went through a round, the `probabilities` map is re-expanded
/// over every original label in criteria order — a finalist keeps the final round's probability, an
/// eliminated option carries `0.0`, so `choice` is still the argmax and the values still sum to 1 —
/// and an `x_tournament` entry records the finalists and round count. The `tournament` metadata key
/// is dropped from the top level. `usage.input_tokens` is overwritten with `total_input_tokens`
/// (summed over every forward pass); the other usage keys stay the final call's.
fn project_tournament(
    mut result: Value,
    large: &[(String, Vec<String>)],
    total_input_tokens: usize,
) -> Value {
    let tournament = result.as_object_mut().and_then(|o| o.remove("tournament"));
    let tmeta = tournament.as_ref().and_then(|v| v.as_object());
    if let Some(answers) = result.get_mut("answers").and_then(|v| v.as_object_mut()) {
        for (qid, labels) in large {
            let Some(meta) = tmeta.and_then(|m| m.get(qid)).and_then(|v| v.as_object()) else {
                continue;
            };
            // A choice that needed no round keeps its one-pass answer untouched (upstream skips it).
            if meta.get("rounds").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
                continue;
            }
            let Some(answer) = answers.get_mut(qid).and_then(|v| v.as_object_mut()) else {
                continue;
            };
            let final_probs = answer
                .get("probabilities")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let mut probabilities = serde_json::Map::with_capacity(labels.len());
            for label in labels {
                let p = final_probs
                    .get(label)
                    .cloned()
                    .unwrap_or_else(|| json!(0.0));
                probabilities.insert(label.clone(), p);
            }
            answer.insert("probabilities".to_string(), Value::Object(probabilities));
            answer.insert(
                "x_tournament".to_string(),
                json!({
                    "finalists": meta.get("labels").cloned().unwrap_or_else(|| json!([])),
                    "rounds": meta.get("rounds").cloned().unwrap_or_else(|| json!(0)),
                }),
            );
        }
    }
    if let Some(usage) = result.get_mut("usage").and_then(|v| v.as_object_mut()) {
        usage.insert("input_tokens".to_string(), json!(total_input_tokens));
    }
    result
}

/// Positive integer from an env var, or `default` when unset, empty or unparseable.
fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

/// Group size for choices answered by elimination, read from `LAYA_TOURNAMENT_GROUP`; 0 is off
/// (upstream `serve._resolve_tournament_group`). See [`parse_tournament_group`] for the rules.
fn resolve_tournament_group() -> usize {
    parse_tournament_group(std::env::var("LAYA_TOURNAMENT_GROUP").ok().as_deref())
}

/// Parse a `LAYA_TOURNAMENT_GROUP` value. Anything unset, empty, unparseable, below 2 or above
/// `MAX_CHOICE_OPTIONS` leaves it off (0), so the one-pass path stays the default. The upper bound
/// keeps the raised cap honest: every choice `MAX_TOURNAMENT_CHOICE_OPTIONS` newly admits has more
/// than `MAX_CHOICE_OPTIONS` options, so it is always over the group and always narrowed (#302).
fn parse_tournament_group(raw: Option<&str>) -> usize {
    match raw.map(str::trim).unwrap_or("") {
        "" => 0,
        s => match s.parse::<usize>() {
            Ok(n) if (2..=MAX_CHOICE_OPTIONS).contains(&n) => n,
            _ => 0,
        },
    }
}

/// Port from `LAYA_PORT`, validated. Exits with a message instead of silently falling back.
fn resolve_port() -> u16 {
    let raw = std::env::var("LAYA_PORT").unwrap_or_else(|_| "8000".to_string());
    match raw.trim().parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => p as u16,
        _ => {
            eprintln!("laya-serve: invalid LAYA_PORT {raw:?}: must be an integer 1-65535");
            std::process::exit(2);
        }
    }
}

/// True when `text` is a filesystem path or a Hub repo id, not a checkpoint name. A slash or
/// backslash is how both a path and an `org/repo` id are written; a leading `.` or `~` is a
/// relative or home path with no slash yet (`./ckpt`, `~/ckpt`). Checkpoint names, aliases and a
/// Jev id such as `jev-1` have none of those (upstream `_names_unpublished_source`).
fn names_unpublished_source(text: &str) -> bool {
    match text.chars().next() {
        None => false,
        Some('.') | Some('~') => true,
        _ => text.contains('/') || text.contains('\\'),
    }
}

/// Map a client's `model` field onto a Laya checkpoint, or `None` to auto-route.
///
/// A Jev id such as `jev-1`, and the bundle id `convaiinnovations/laya`, stay `None`: both mean
/// "let the Router choose". A path or an unpublished Hub repo id is a different miss — swallowing it
/// would answer with whichever checkpoint routing picked, a wrong answer — so it is a 422 instead
/// (upstream `_resolve_model` / #919).
fn resolve_model(model: Option<&str>) -> Result<Option<String>, (StatusCode, Json<Value>)> {
    let Some(model) = model else {
        return Ok(None);
    };
    let text = model.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let published = match text.to_lowercase().as_str() {
        "convaiinnovations/laya-multilingual" => Some("multilingual"),
        "convaiinnovations/laya-typed-decisions" => Some("typed-decisions"),
        _ => None,
    };
    if let Some(p) = published {
        return Ok(Some(p.to_string()));
    }
    // The root bundle is the one Hub id whose documented meaning is auto-route, not a pin. It
    // contains a slash, so the path check below would otherwise refuse it.
    if text.eq_ignore_ascii_case(BUNDLE_REPO) {
        return Ok(None);
    }
    // A Jev client's model id (e.g. "jev-1") is expected to miss; treat as auto-route. A path or an
    // unpublished Hub id is not that miss: the caller named a checkpoint this server cannot load.
    match normalise_name(text) {
        Ok(key) if KNOWN_MODELS.contains(&key.as_str()) => Ok(Some(key)),
        Ok(_) => Ok(None),
        Err(e) => {
            if names_unpublished_source(text) {
                Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("{e}, or omit model to let the router choose"),
                ))
            } else {
                Ok(None)
            }
        }
    }
}

/// A `lang` / `lang_guess` body value: a language code string, or absent/null. A non-string (a
/// JSON bool or number) is a 422 — routing stringifies the hint through `english_from_code`, so a
/// bare `true` would become the real code `"true"` and decide the checkpoint. Explicit `null` stays
/// "no hint", which lets a deployment's `Router(lang_guess=...)` answer (upstream
/// `_validate_language_param`).
fn validate_lang(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, (StatusCode, Json<Value>)> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("{key} must be a language code string such as \"de\", or null"),
        )),
    }
}

fn build_router() -> Router {
    let device = std::env::var("LAYA_DEVICE").ok().filter(|s| !s.is_empty());
    // Checkpoints kept resident at once (upstream `LAYA_MAX_LOADED`, default 2 — the number
    // automatic routing picks between). A value below what routing can choose reloads one per
    // switch; `preload` still raises the cap to hold whatever it builds, so this never evicts a
    // preloaded checkpoint. Unset/invalid falls back to the default, like `LAYA_MAX_CONCURRENT`.
    let max_loaded = env_usize("LAYA_MAX_LOADED", RouterOptions::default().max_loaded);
    let mut opts = RouterOptions {
        device,
        auto_task_detection: env_bool("LAYA_AUTO_TASK", false),
        max_loaded,
        ..Default::default()
    };
    // The routing fallback for the two states carrying no language evidence (no letters, or Latin
    // script too short to identify). A deployment whose traffic is mostly non-English sets this; a
    // typo is fatal, since a silently-ignored value would keep routing those states to the wrong
    // checkpoint (upstream `LAYA_DEFAULT_MODEL` / `_default_model_option`).
    if let Some(raw) = std::env::var("LAYA_DEFAULT_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        match normalise_name(&raw) {
            Ok(name) => opts.default = name,
            Err(e) => {
                eprintln!(
                    "laya-serve: invalid LAYA_DEFAULT_MODEL {:?}: {e}",
                    raw.trim()
                );
                std::process::exit(2);
            }
        }
    }
    let router = Router::new(opts).expect("router options");
    if env_bool("LAYA_PRELOAD", true) {
        let models_env = std::env::var("LAYA_MODELS").unwrap_or_default();
        let names: Vec<&str> = models_env
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let result = if names.is_empty() {
            router.preload(None)
        } else {
            router.preload(Some(&names))
        };
        if let Err(e) = result {
            eprintln!("laya-serve: preload failed: {e}");
        }
    }
    router
}

async fn health(State(app): State<AppState>, headers: HeaderMap) -> Json<Value> {
    // On a server locked down with `LAYA_API_KEY`, the full payload names the resident checkpoints
    // and the device — reconnaissance an unauthenticated caller should not get. Without a valid
    // bearer, answer liveness only. A wrong/absent bearer still gets 200 (not 401) so a plain
    // liveness probe does not fail; with no key configured the full payload is open as before
    // (upstream GET /health bearer gate / #812).
    if let Some(key) = &app.api_key {
        let expected = format!("Bearer {key}");
        let supplied = headers
            .get("authorization")
            .map(|v| v.as_bytes())
            .unwrap_or(b"");
        if !constant_time_eq(supplied, expected.as_bytes()) {
            return Json(json!({ "status": "ok" }));
        }
    }
    Json(json!({
        "status": "ok",
        "loaded": app.router.loaded(),
        "device": app.device,
    }))
}

async fn systemone(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    // Optional bearer auth. Compared in constant time over raw bytes so no header a client can
    // send leaks the token by timing or crashes the comparison.
    if let Some(key) = &app.api_key {
        let expected = format!("Bearer {key}");
        let supplied = headers
            .get("authorization")
            .map(|v| v.as_bytes())
            .unwrap_or(b"");
        if !constant_time_eq(supplied, expected.as_bytes()) {
            return Err(err(
                StatusCode::UNAUTHORIZED,
                "invalid or missing bearer token",
            ));
        }
    }

    // Bound in-flight requests without blocking: a non-blocking acquire, held to the end of the
    // handler, so excess load is shed with 503 rather than buffering behind the inference gate.
    let _admit = match app.admission.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            // Retry-After tells well-behaved clients when a slot is likely free: admission turns
            // over at inference speed, so one second is the honest hint (upstream 503 Retry-After).
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", "1".parse().unwrap());
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                headers,
                Json(json!({ "detail": "server busy; too many concurrent requests" })),
            )
                .into_response());
        }
    };

    let obj = match body.as_object() {
        Some(o) if o.contains_key("questions") => o,
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "request body must be an object with a 'questions' field",
            ))
        }
    };
    // A missing or null `state` is the one input where a silent wrong answer is worse than an
    // error: with no state the engine would answer about the literal text "null" at high
    // confidence, and the caller has no signal anything went wrong. A string state ("null", "")
    // is the caller's business and is left alone (upstream #375/serve-require-state).
    match obj.get("state") {
        None | Some(Value::Null) => {
            return Err(err(StatusCode::BAD_REQUEST, "'state' is required"))
        }
        _ => {}
    }
    let state = obj.get("state").cloned().unwrap_or(Value::Null);
    let questions_val = obj.get("questions").cloned().unwrap_or(Value::Null);
    let questions: laya::Questions = match questions_val {
        Value::Object(m) => m.into_iter().collect(),
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "'questions' must be an object",
            ))
        }
    };

    // Reject oversized inference requests before tokenization (413).
    if questions.len() > MAX_QUESTIONS {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "too many questions ({} > {})",
                questions.len(),
                MAX_QUESTIONS
            ),
        ));
    }
    // Bound the number of answer options across questions: each choice label or score level becomes
    // one collated tensor row, so an unbounded count amplifies a small request (upstream #335).
    // With `LAYA_TOURNAMENT_GROUP` set, a choice is narrowed in groups rather than read in one pass,
    // so the choice cap is the larger `MAX_TOURNAMENT_CHOICE_OPTIONS` instead (#302).
    let max_choice_options = if app.tournament_group > 0 {
        MAX_TOURNAMENT_CHOICE_OPTIONS
    } else {
        MAX_CHOICE_OPTIONS
    };
    let mut total_options = 0usize;
    for (qid, question) in &questions {
        let Some(q) = question.as_object() else {
            continue;
        };
        let qtype = q.get("type").and_then(|v| v.as_str());
        let count = match q.get("criteria") {
            Some(Value::Object(m)) => m.len(),
            Some(Value::Array(a)) => a.len(),
            _ => continue,
        };
        match qtype {
            Some("choice") => {
                total_options += count;
                if count > max_choice_options {
                    return Err(err(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        &format!(
                            "too many choice options for {qid:?} ({count} > {max_choice_options})"
                        ),
                    ));
                }
            }
            // A score question takes its levels as a list; a dict criteria is not a level list.
            Some("score") if matches!(q.get("criteria"), Some(Value::Array(_))) => {
                total_options += count;
                if count > MAX_SCORE_LEVELS {
                    return Err(err(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        &format!(
                            "too many score levels for {qid:?} ({count} > {MAX_SCORE_LEVELS})"
                        ),
                    ));
                }
                // A null level is a hole in the rubric: the answer's `legend` would carry
                // `{"<i>": null}`, which a Jev client's schema refuses to parse (#302). Reject it as
                // a malformed request rather than answering 200 with an unparseable legend.
                if let Some(Value::Array(levels)) = q.get("criteria") {
                    if let Some(idx) = levels.iter().position(|v| v.is_null()) {
                        return Err(err(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            &format!(
                                "score question {qid:?} has a null level at index {idx}; \
                                 give every level a description"
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if total_options > MAX_TOTAL_OPTIONS {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "too many answer options across questions ({total_options} > {MAX_TOTAL_OPTIONS})"
            ),
        ));
    }
    // Measured on the text the tokenizer receives — `serialize_state`, which is the string itself
    // for a string state and `", "`/`": "`-separated JSON for a dict or list — not `to_string()`'s
    // compact form. The cap therefore counts the same characters the model is charged for, in both
    // directions (upstream: measure the state limit on the tokenizer's text).
    let state_len = match &state {
        Value::String(s) => s.chars().count(),
        other => laya::common::serialize_state(other).chars().count(),
    };
    if state_len > MAX_STATE_CHARS {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "state too large ({} > {} chars)",
                state_len, MAX_STATE_CHARS
            ),
        ));
    }

    let model = resolve_model(obj.get("model").and_then(|v| v.as_str()))?;
    // Kept for the server-side log below: `model` itself is moved into the blocking task.
    let model_log = model.clone();

    // Forward the routing controls a JSON body can carry, each only when the client sent it, so an
    // absent field still inherits what the Router was built with (upstream BODY_CONTROLS). `hooks`,
    // `min_confidence` and a token budget have no counterpart on the single-request Rust surface.
    // `task` is honoured, rejecting an unknown one as 422 the way core's ValueError surfaces
    // upstream; `lang` / `lang_guess` must be a code string, since a bool or number would be
    // stringified into a real, non-English code and decide the checkpoint.
    let task = match obj.get("task") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let normalized = if s.to_lowercase().replace('-', "_") == "typed_decisions" {
                "typed-decisions".to_string()
            } else {
                s.clone()
            };
            if normalise_name(&normalized).is_err() {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("unknown task {s:?}"),
                ));
            }
            Some(s.clone())
        }
        Some(_) => {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "'task' must be a string",
            ))
        }
    };
    let lang = validate_lang(obj, "lang")?;
    let lang_guess = validate_lang(obj, "lang_guess")?;

    let router = app.router.clone();
    // Whether this request answers a large choice by elimination (`LAYA_TOURNAMENT_GROUP`), and the
    // full label set of each such choice for re-expanding its probabilities afterwards (#302). When
    // the knob is off, or no choice is large enough (or one cannot be narrowed), this is `None` and
    // the one-pass `Router::predict` path runs exactly as before.
    let tournament = if app.tournament_group > 0 {
        tournament_targets(&questions, app.tournament_group)
    } else {
        None
    };
    let tournament_group = app.tournament_group;
    // One forward pass at a time: hold a permit across the blocking inference.
    let permit = app
        .gate
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let t0 = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let hints = RouteHints {
            model: model.as_deref(),
            task: task.as_deref(),
            lang: lang.as_deref(),
            lang_guess: lang_guess.as_deref(),
        };
        match tournament {
            Some(large) => {
                // Route and load the checkpoint once; every elimination round and the final
                // answer run on it. Routing is a pure function of the (unchanged) state, so this
                // matches re-routing each round, which is how upstream drives the tournament.
                let decision = router.route(&state, Some(&questions), &hints)?;
                let agent = router.load(&decision.model)?;
                let (out, total_input) = laya::shortlist::predict_tournament_with_usage(
                    &agent,
                    &state,
                    &questions,
                    tournament_group,
                )?;
                let mut out = project_tournament(out, &large, total_input);
                // `predict_tournament` answers through `system_one`, which carries no routing;
                // attach it here so the response matches the one-pass path's `routing` field.
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("routing".to_string(), decision.to_json());
                }
                Ok(out)
            }
            None => router
                .predict(&state, &questions, &hints)
                .map(|r| r.to_json()),
        }
    })
    .await;
    let infer_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(Ok(v)) => {
            let v = if app.jev_strict {
                project_jev_strict(v)
            } else {
                add_jev_confidence(v)
            };
            // Expose the inference time the same way upstream `laya-serve` does, so a client can
            // read it without a separate timing endpoint.
            let mut headers = HeaderMap::new();
            headers.insert(
                "server-timing",
                format!("inference;dur={infer_ms:.2}").parse().unwrap(),
            );
            headers.insert(
                "x-inference-time-ms",
                format!("{infer_ms:.2}").parse().unwrap(),
            );
            Ok((headers, Json(v)).into_response())
        }
        // Question-validation errors name the question and what to fix: safe to return.
        Ok(Err(e @ LayaError::InvalidQuestion(_))) => {
            Err(err(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()))
        }
        // Anything else (download, tokenizer, model, OOM) may carry paths or weights details;
        // never leak it to clients — but log it server-side, the only place the actual cause can
        // appear once the client sees a fixed 500.
        Ok(Err(e)) => {
            tracing::error!(error = %e, model = ?model_log, "inference failed");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "inference failed"))
        }
        Err(e) => {
            tracing::error!(error = %e, model = ?model_log, "inference task panicked");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "inference failed"))
        }
    }
}

fn err(code: StatusCode, detail: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "detail": detail })))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LAYA_LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let device = std::env::var("LAYA_DEVICE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "cpu".to_string());
    let max_concurrent = env_usize("LAYA_MAX_CONCURRENT", DEFAULT_MAX_CONCURRENT);
    let app_state = AppState {
        router: Arc::new(build_router()),
        gate: Arc::new(Semaphore::new(1)),
        admission: Arc::new(Semaphore::new(max_concurrent)),
        api_key: std::env::var("LAYA_API_KEY").ok().filter(|s| !s.is_empty()),
        device,
        jev_strict: env_bool("LAYA_JEV_STRICT", false),
        tournament_group: resolve_tournament_group(),
    };

    let app = AxumRouter::new()
        .route("/health", get(health))
        .route("/v1/systemone", post(systemone))
        // Refuse to buffer a body larger than the cap, whatever the client's declared length.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app_state);

    let host = std::env::var("LAYA_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = resolve_port();
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("valid bind address");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("laya-serve listening on http://{addr}");
    axum::serve(listener, app).await.expect("server");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpublished_source_detects_paths_and_repos() {
        assert!(names_unpublished_source("./ckpt"));
        assert!(names_unpublished_source("~/ckpt"));
        assert!(names_unpublished_source("org/secret-model"));
        assert!(names_unpublished_source("C:\\models\\laya"));
        // Checkpoint names, aliases and Jev ids are not path-like.
        assert!(!names_unpublished_source("jev-1"));
        assert!(!names_unpublished_source("multilingual"));
        assert!(!names_unpublished_source(""));
    }

    #[test]
    fn resolve_model_maps_published_and_auto_routes() {
        assert_eq!(resolve_model(None).unwrap(), None);
        assert_eq!(resolve_model(Some("")).unwrap(), None);
        // Published Hub ids map to checkpoint names.
        assert_eq!(
            resolve_model(Some("convaiinnovations/laya-multilingual")).unwrap(),
            Some("multilingual".to_string())
        );
        // Aliases resolve.
        assert_eq!(
            resolve_model(Some("ml")).unwrap(),
            Some("multilingual".to_string())
        );
        // The bundle id and a bare Jev id auto-route (None).
        assert_eq!(resolve_model(Some("convaiinnovations/laya")).unwrap(), None);
        assert_eq!(resolve_model(Some("jev-1")).unwrap(), None);
    }

    #[test]
    fn resolve_model_refuses_path_like_misses() {
        let (code, _) = resolve_model(Some("./my-checkpoint")).unwrap_err();
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        let (code, _) = resolve_model(Some("org/unpublished")).unwrap_err();
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn jev_strict_drops_non_contract_fields() {
        let full = json!({
            "model": "laya-rl-agent",
            "routing": {"model": "english"},
            "answers": {
                "q1": {"type": "choice", "choice": "a", "confidence": 0.9,
                       "answer_confidence": 0.8, "probabilities": {"a": 0.9, "b": 0.1},
                       "action": {"act_probability": 0.5}},
                "q2": {"type": "noul", "noul": 0.7, "confidence": 0.7,
                       "answer_confidence": 0.7, "action": {"act_probability": 0.1}},
            },
            "usage": {"input_tokens": 12, "output_tokens": 3, "truncated": false},
        });
        let strict = project_jev_strict(full);
        let obj = strict.as_object().unwrap();
        assert!(!obj.contains_key("routing"));
        assert_eq!(
            obj["usage"],
            json!({"input_tokens": 12, "output_tokens": 3})
        );
        let q1 = &strict["answers"]["q1"];
        assert!(q1.get("action").is_none());
        assert!(q1.get("answer_confidence").is_none());
        assert_eq!(q1["choice"], json!("a"));
        let q2 = &strict["answers"]["q2"];
        // noul keeps only type + noul.
        assert_eq!(q2, &json!({"type": "noul", "noul": 0.7}));
    }

    #[test]
    fn add_jev_confidence_annotates_choice_and_score_only() {
        let full = json!({
            "model": "english",
            "answers": {
                // choice: (max - 1/3) / (1 - 1/3) over 0.9519 / 0.0327 / 0.0154 -> 0.9278
                "queue": {"type": "choice", "choice": "billing", "confidence": 0.797,
                          "probabilities": {"billing": 0.9519, "tech": 0.0327, "other": 0.0154}},
                // score: 3 levels, mode 1, uniform D = 2/3, spread 0.35 -> 1 - 0.35/(2/3) = 0.475
                "urgency": {"type": "score", "score": 1.0, "confidence": 0.1925,
                            "probabilities": {"0": 0.2, "1": 0.65, "2": 0.15}},
                "threat": {"type": "noul", "noul": 0.2, "confidence": 0.6},
            },
        });
        let out = add_jev_confidence(full);
        assert_eq!(out["answers"]["queue"]["x_jev_confidence"], json!(0.9278));
        assert_eq!(out["answers"]["urgency"]["x_jev_confidence"], json!(0.475));
        // Laya's own numbers are untouched, and a noul answer gets no Jev confidence.
        assert_eq!(out["answers"]["queue"]["confidence"], json!(0.797));
        assert!(out["answers"]["threat"].get("x_jev_confidence").is_none());
    }

    #[test]
    fn parse_tournament_group_bounds() {
        // Unset / empty / unparseable / below 2 / above the one-pass cap all leave it off.
        assert_eq!(parse_tournament_group(None), 0);
        assert_eq!(parse_tournament_group(Some("")), 0);
        assert_eq!(parse_tournament_group(Some("  ")), 0);
        assert_eq!(parse_tournament_group(Some("abc")), 0);
        assert_eq!(parse_tournament_group(Some("1")), 0);
        assert_eq!(parse_tournament_group(Some("0")), 0);
        assert_eq!(
            parse_tournament_group(Some(&(MAX_CHOICE_OPTIONS + 1).to_string())),
            0
        );
        // In range (2..=MAX_CHOICE_OPTIONS), including both ends, is honoured.
        assert_eq!(parse_tournament_group(Some("2")), 2);
        assert_eq!(parse_tournament_group(Some(" 16 ")), 16);
        assert_eq!(
            parse_tournament_group(Some(&MAX_CHOICE_OPTIONS.to_string())),
            MAX_CHOICE_OPTIONS
        );
    }

    fn choice(options: usize) -> Value {
        let criteria: serde_json::Map<String, Value> = (0..options)
            .map(|i| (format!("opt{i}"), json!(format!("option {i}"))))
            .collect();
        json!({"type": "choice", "criteria": Value::Object(criteria)})
    }

    #[test]
    fn tournament_targets_picks_only_large_narrowable_choices() {
        // No choice over the group -> one-pass (None).
        let mut qs = laya::Questions::new();
        qs.insert("small".into(), choice(3));
        qs.insert("note".into(), json!({"type": "noul"}));
        assert!(tournament_targets(&qs, 4).is_none());

        // One large choice -> Some, carrying its full label set in criteria order.
        let mut qs = laya::Questions::new();
        qs.insert("big".into(), choice(10));
        qs.insert("small".into(), choice(2));
        let large = tournament_targets(&qs, 4).expect("a large choice");
        assert_eq!(large.len(), 1);
        assert_eq!(large[0].0, "big");
        assert_eq!(large[0].1.len(), 10);
        assert_eq!(large[0].1[0], "opt0");

        // A large choice carrying an option_order cannot be narrowed -> whole request one-pass.
        let mut big = choice(10);
        big.as_object_mut()
            .unwrap()
            .insert("option_order".into(), json!((0..10).collect::<Vec<_>>()));
        let mut qs = laya::Questions::new();
        qs.insert("big".into(), big);
        assert!(tournament_targets(&qs, 4).is_none());

        // A large array-criteria choice with a non-string label -> one-pass.
        let mut qs = laya::Questions::new();
        qs.insert(
            "big".into(),
            json!({"type": "choice", "criteria": [1, 2, 3, 4, 5]}),
        );
        assert!(tournament_targets(&qs, 4).is_none());
    }

    #[test]
    fn project_tournament_reexpands_probabilities_and_records_rounds() {
        // The library result: `big` went through a round, its probabilities are over the two
        // finalists only, and the top level carries the `tournament` metadata.
        let result = json!({
            "model": "english",
            "answers": {
                "big": {"type": "choice", "choice": "opt2", "confidence": 0.8,
                        "probabilities": {"opt2": 0.7, "opt5": 0.3}},
            },
            "usage": {"input_tokens": 40, "output_tokens": 8},
            "tournament": {"big": {"labels": ["opt2", "opt5"], "n": 6, "rounds": 1}},
        });
        let labels: Vec<String> = (0..6).map(|i| format!("opt{i}")).collect();
        let large = vec![("big".to_string(), labels)];
        let out = project_tournament(result, &large, 123);

        let probs = out["answers"]["big"]["probabilities"].as_object().unwrap();
        // Every original label is named, in criteria order; eliminated options carry 0.0.
        assert_eq!(probs.len(), 6);
        assert_eq!(probs["opt2"], json!(0.7));
        assert_eq!(probs["opt5"], json!(0.3));
        assert_eq!(probs["opt0"], json!(0.0));
        assert_eq!(probs["opt4"], json!(0.0));
        // The values still sum to 1 and `choice` is still the argmax.
        assert_eq!(out["answers"]["big"]["choice"], json!("opt2"));
        // x_tournament records the finalists and the round count.
        assert_eq!(
            out["answers"]["big"]["x_tournament"]["finalists"],
            json!(["opt2", "opt5"])
        );
        assert_eq!(out["answers"]["big"]["x_tournament"]["rounds"], json!(1));
        // input_tokens is the summed total; output_tokens stays the final call's; tournament gone.
        assert_eq!(out["usage"]["input_tokens"], json!(123));
        assert_eq!(out["usage"]["output_tokens"], json!(8));
        assert!(out.get("tournament").is_none());
    }

    #[test]
    fn project_tournament_leaves_a_passthrough_choice_untouched() {
        // `rounds: 0` means the choice fit in one pass; its one-pass answer is kept as is, with no
        // probability re-expansion and no x_tournament.
        let result = json!({
            "model": "english",
            "answers": {
                "big": {"type": "choice", "choice": "opt0", "confidence": 0.9,
                        "probabilities": {"opt0": 0.6, "opt1": 0.4}},
            },
            "usage": {"input_tokens": 20, "output_tokens": 4},
            "tournament": {"big": {"labels": ["opt0", "opt1"], "n": 2, "rounds": 0}},
        });
        let large = vec![("big".to_string(), vec!["opt0".into(), "opt1".into()])];
        let out = project_tournament(result, &large, 20);
        assert_eq!(
            out["answers"]["big"]["probabilities"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert!(out["answers"]["big"].get("x_tournament").is_none());
    }
}
