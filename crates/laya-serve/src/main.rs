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
//! | `LAYA_API_KEY`   | if set, require `Authorization: Bearer <it>`         | (none)  |

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
use laya::router::{normalise_name, RouteHints, Router, RouterOptions};
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
    api_key: Option<String>,
    device: String,
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

/// Map a client's `model` field onto a Laya checkpoint, or `None` to auto-route.
fn resolve_model(model: Option<&str>) -> Option<String> {
    let model = model?;
    if model.is_empty() {
        return None;
    }
    let published = match model.trim().to_lowercase().as_str() {
        "convaiinnovations/laya-multilingual" => Some("multilingual"),
        "convaiinnovations/laya-typed-decisions" => Some("typed-decisions"),
        _ => None,
    };
    if let Some(p) = published {
        return Some(p.to_string());
    }
    // A Jev client's model id (e.g. "jev-1") is expected to miss; treat as auto-route.
    match normalise_name(model) {
        Ok(key) if KNOWN_MODELS.contains(&key.as_str()) => Some(key),
        _ => None,
    }
}

fn build_router() -> Router {
    let device = std::env::var("LAYA_DEVICE").ok().filter(|s| !s.is_empty());
    let router = Router::new(RouterOptions {
        device,
        auto_task_detection: env_bool("LAYA_AUTO_TASK", false),
        ..Default::default()
    })
    .expect("router options");
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

async fn health(State(app): State<AppState>) -> Json<Value> {
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

    let obj = match body.as_object() {
        Some(o) if o.contains_key("questions") => o,
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "request body must be an object with a 'questions' field",
            ))
        }
    };
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
    let state_len = match &state {
        Value::String(s) => s.chars().count(),
        other => other.to_string().chars().count(),
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

    let model = resolve_model(obj.get("model").and_then(|v| v.as_str()));
    // Kept for the server-side log below: `model` itself is moved into the blocking task.
    let model_log = model.clone();

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
            ..Default::default()
        };
        router
            .predict(&state, &questions, &hints)
            .map(|r| r.to_json())
    })
    .await;
    let infer_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(Ok(v)) => {
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
    let app_state = AppState {
        router: Arc::new(build_router()),
        gate: Arc::new(Semaphore::new(1)),
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
