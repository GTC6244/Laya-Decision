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
//! | `LAYA_DEFAULT_MODEL` | fallback checkpoint when a state carries no language evidence; aliases like `ml` work | (english) |
//! | `LAYA_API_KEY`   | if set, require `Authorization: Bearer <it>`         | (none)  |
//! | `LAYA_MAX_CONCURRENT` | in-flight requests admitted before shedding 503 | 16      |
//! | `LAYA_JEV_STRICT` | if set, serve the strict Jev wire contract: no root `routing`, no per-answer `action` / `answer_confidence`, no `confidence` on noul answers | 0 |

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
/// backslash is how both a path and an `org/repo` id are written; a leading `.`/`~` is a relative
/// or home path with no slash yet (upstream `_names_unpublished_source`).
fn names_unpublished_source(text: &str) -> bool {
    match text.chars().next() {
        None => false,
        Some('.') | Some('~') => true,
        _ => text.contains('/') || text.contains('\\'),
    }
}

/// Map a client's `model` field onto a Laya checkpoint, or `None` to auto-route.
///
/// A Jev id such as `jev-1`, and the bundle id `convaiinnovations/laya`, stay `None` ("let the
/// router choose"). A path or an unpublished Hub repo id is a different miss: auto-routing it would
/// answer with whichever checkpoint routing picked, a wrong answer, so that request is a 422
/// instead (upstream `_resolve_model` / #919).
fn resolve_model(model: Option<&str>) -> Result<Option<String>, (StatusCode, Json<Value>)> {
    let Some(model) = model else {
        return Ok(None);
    };
    if model.is_empty() {
        return Ok(None);
    }
    let text = model.trim();
    let published = match text.to_lowercase().as_str() {
        "convaiinnovations/laya-multilingual" => Some("multilingual"),
        "convaiinnovations/laya-typed-decisions" => Some("typed-decisions"),
        _ => None,
    };
    if let Some(p) = published {
        return Ok(Some(p.to_string()));
    }
    // The root bundle is the one Hub id whose documented meaning is auto-route, not a pin; it
    // contains a slash, so the path check below would otherwise refuse it.
    if text.eq_ignore_ascii_case(BUNDLE_REPO) {
        return Ok(None);
    }
    // A Jev client's model id (e.g. "jev-1") is expected to miss; treat as auto-route. A path or an
    // unpublished Hub id is not that miss — the caller named a checkpoint this server cannot load.
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
    let mut options = RouterOptions {
        device,
        auto_task_detection: env_bool("LAYA_AUTO_TASK", false),
        max_loaded,
        ..Default::default()
    };
    // Routing fallback for a state that carries no language evidence at all (upstream
    // `LAYA_DEFAULT_MODEL`). Left at the Router's own default when unset/blank, so the value cannot
    // drift; an unknown name is a configuration error, so exit with the message like `resolve_port`
    // rather than silently routing ambiguous states to a checkpoint that cannot read them.
    if let Ok(raw) = std::env::var("LAYA_DEFAULT_MODEL") {
        let name = raw.trim();
        if !name.is_empty() {
            match normalise_name(name) {
                Ok(resolved) => options.default = resolved,
                Err(e) => {
                    eprintln!("laya-serve: invalid LAYA_DEFAULT_MODEL {name:?}: {e}");
                    std::process::exit(2);
                }
            }
        }
    }
    let router = Router::new(options).expect("router options");
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

/// Whether a request carries the configured bearer. True when no key is set.
fn authorized(api_key: &Option<String>, authorization: Option<&[u8]>) -> bool {
    match api_key {
        None => true,
        Some(key) => {
            let expected = format!("Bearer {key}");
            constant_time_eq(authorization.unwrap_or(b""), expected.as_bytes())
        }
    }
}

async fn health(State(app): State<AppState>, headers: HeaderMap) -> Json<Value> {
    // Liveness stays open, because every shipped probe reads it without a credential and the page
    // promises as much. What is not open on a locked-down deployment is the detail below it:
    // resident checkpoint names and the host device state. An unauthenticated caller gets the
    // status and nothing else (upstream #812).
    if !authorized(
        &app.api_key,
        headers.get("authorization").map(|v| v.as_bytes()),
    ) {
        return Json(json!({ "status": "ok" }));
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
    if !authorized(
        &app.api_key,
        headers.get("authorization").map(|v| v.as_bytes()),
    ) {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "invalid or missing bearer token",
        ));
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
                if count > MAX_CHOICE_OPTIONS {
                    return Err(err(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        &format!(
                            "too many choice options for {qid:?} ({count} > {MAX_CHOICE_OPTIONS})"
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
                // `{"<i>": null}`, which a Jev client's schema refuses to parse. Reject it as a
                // malformed request rather than answering 200 with an unparseable legend
                // (upstream #302).
                if let Some(Value::Array(levels)) = q.get("criteria") {
                    if let Some(i) = levels.iter().position(|v| v.is_null()) {
                        return Err(err(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            &format!(
                                "score question {qid:?} has a null level at index {i}; give every \
                                 level a description"
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
        router
            .predict(&state, &questions, &hints)
            .map(|r| r.to_json())
    })
    .await;
    let infer_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(Ok(v)) => {
            // Strict Jev wire contract: drop the Laya extensions a contract-validating client may
            // reject (root `routing`, per-answer `action`/`answer_confidence`, noul `confidence`).
            let v = if env_bool("LAYA_JEV_STRICT", false) {
                project_jev_strict(v)
            } else {
                v
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

/// Project a result onto the strict Jev wire contract (`LAYA_JEV_STRICT`).
///
/// The Jev `/v1/systemone` response defines exactly three top-level fields (`model`, `answers`,
/// `usage`), and each answer carries only its type's fields: choice = `choice` + `probabilities` +
/// `confidence`, score = `score` + `probabilities` + `confidence` + `legend`, noul = `noul` only.
/// Laya's full payload adds more — a root `routing` report, a per-answer `action` head and the
/// calibrated `answer_confidence`, and a `confidence` on noul answers — which a client validating
/// the response against the contract with no extra fields may reject. This keeps only the
/// contracted keys. Nothing is recomputed, and an answer of an unknown shape passes through
/// unchanged (upstream `_project_jev_strict`).
fn project_jev_strict(result: Value) -> Value {
    let Some(obj) = result.as_object() else {
        return result;
    };
    let field = |answer: &Value, key: &str| answer.get(key).cloned().unwrap_or(Value::Null);
    let mut answers_out = serde_json::Map::new();
    if let Some(Value::Object(answers)) = obj.get("answers") {
        for (qid, answer) in answers {
            let kind = answer.get("type").and_then(|t| t.as_str());
            let projected = match kind {
                Some("choice") => json!({
                    "type": "choice",
                    "choice": field(answer, "choice"),
                    "confidence": field(answer, "confidence"),
                    "probabilities": field(answer, "probabilities"),
                }),
                Some("score") => json!({
                    "type": "score",
                    "score": field(answer, "score"),
                    "confidence": field(answer, "confidence"),
                    "probabilities": field(answer, "probabilities"),
                    "legend": field(answer, "legend"),
                }),
                Some("noul") => json!({
                    "type": "noul",
                    "noul": field(answer, "noul"),
                }),
                _ => answer.clone(),
            };
            answers_out.insert(qid.clone(), projected);
        }
    }
    let usage = obj.get("usage").and_then(|u| u.as_object());
    let token = |key: &str| {
        usage
            .and_then(|u| u.get(key))
            .cloned()
            .unwrap_or_else(|| json!(0))
    };
    json!({
        "model": obj.get("model").cloned().unwrap_or(Value::Null),
        "answers": Value::Object(answers_out),
        "usage": {"input_tokens": token("input_tokens"), "output_tokens": token("output_tokens")},
    })
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
    fn resolve_model_published_and_aliases() {
        assert_eq!(resolve_model(None).unwrap(), None);
        assert_eq!(resolve_model(Some("")).unwrap(), None);
        assert_eq!(
            resolve_model(Some("convaiinnovations/laya-multilingual")).unwrap(),
            Some("multilingual".to_string())
        );
        // A checkpoint alias resolves; the bundle id and a Jev id auto-route.
        assert_eq!(
            resolve_model(Some("ml")).unwrap(),
            Some("multilingual".to_string())
        );
        assert_eq!(resolve_model(Some("convaiinnovations/laya")).unwrap(), None);
        assert_eq!(resolve_model(Some("jev-1")).unwrap(), None);
    }

    #[test]
    fn resolve_model_refuses_unpublished_path_like() {
        // A path or unpublished Hub id names a checkpoint the server cannot load: 422, not a route.
        for bad in ["./ckpt", "~/ckpt", "some/other-repo", "a\\b"] {
            let e = resolve_model(Some(bad)).unwrap_err();
            assert_eq!(e.0, StatusCode::UNPROCESSABLE_ENTITY);
        }
        // A bare unknown word is still a Jev-style miss, not a path: auto-route.
        assert_eq!(resolve_model(Some("mystery-model")).unwrap(), None);
    }

    #[test]
    fn names_unpublished_source_cases() {
        assert!(names_unpublished_source("./x"));
        assert!(names_unpublished_source("~/x"));
        assert!(names_unpublished_source("org/repo"));
        assert!(names_unpublished_source("a\\b"));
        assert!(!names_unpublished_source("english"));
        assert!(!names_unpublished_source(""));
    }

    #[test]
    fn jev_strict_drops_extensions() {
        let full = json!({
            "model": "laya-rl-agent",
            "routing": {"model": "english", "reason": "x"},
            "answers": {
                "a": {"type": "choice", "choice": "yes", "confidence": 0.9,
                      "probabilities": {"yes": 0.9, "no": 0.1},
                      "answer_confidence": 0.8, "action": {"act_probability": 0.3}},
                "b": {"type": "noul", "noul": 0.7, "confidence": 0.7,
                      "answer_confidence": 0.6, "action": {"act_probability": 0.2}},
                "c": {"type": "score", "score": 1.5, "confidence": 0.5,
                      "probabilities": {"0": 0.5, "1": 0.5}, "legend": {"0": "low", "1": "high"},
                      "answer_confidence": 0.5, "action": {"act_probability": 0.1}},
            },
            "usage": {"input_tokens": 12, "output_tokens": 0, "windows": 1},
        });
        let strict = project_jev_strict(full);
        // Root routing is gone; usage is reduced to the two token counts.
        assert!(strict.get("routing").is_none());
        assert_eq!(
            strict["usage"],
            json!({"input_tokens": 12, "output_tokens": 0})
        );
        // Choice keeps its four contracted fields, drops action/answer_confidence.
        let a = &strict["answers"]["a"];
        assert_eq!(a["choice"], json!("yes"));
        assert!(a.get("action").is_none() && a.get("answer_confidence").is_none());
        // noul is reduced to type + noul only.
        assert_eq!(strict["answers"]["b"], json!({"type": "noul", "noul": 0.7}));
        // score keeps its legend and confidence, drops the extensions.
        let c = &strict["answers"]["c"];
        assert!(c.get("legend").is_some() && c.get("confidence").is_some());
        assert!(c.get("action").is_none() && c.get("answer_confidence").is_none());
    }

    #[test]
    fn jev_strict_passes_unknown_shape_through() {
        let r = json!({"model": "m", "answers": {"q": {"type": "weird", "v": 1}}, "usage": {}});
        let strict = project_jev_strict(r);
        assert_eq!(strict["answers"]["q"], json!({"type": "weird", "v": 1}));
        assert_eq!(
            strict["usage"],
            json!({"input_tokens": 0, "output_tokens": 0})
        );
    }
}
