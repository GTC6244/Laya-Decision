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
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router as AxumRouter,
};
use laya::router::{normalise_name, RouteHints, Router, RouterOptions};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

/// Checkpoint names the router understands.
const KNOWN_MODELS: [&str; 3] = ["english", "multilingual", "typed-decisions"];

#[derive(Clone)]
struct AppState {
    router: Arc<Router>,
    gate: Arc<Semaphore>,
    api_key: Option<String>,
    device: String,
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
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
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Optional bearer auth.
    if let Some(key) = &app.api_key {
        let ok = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == format!("Bearer {key}"))
            .unwrap_or(false);
        if !ok {
            return Err(err(StatusCode::UNAUTHORIZED, "invalid or missing bearer token"));
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
        _ => return Err(err(StatusCode::BAD_REQUEST, "'questions' must be an object")),
    };
    let model = resolve_model(obj.get("model").and_then(|v| v.as_str()));

    let router = app.router.clone();
    // One forward pass at a time: hold a permit across the blocking inference.
    let permit = app
        .gate
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let hints = RouteHints {
            model: model.as_deref(),
            ..Default::default()
        };
        router.predict(&state, &questions, &hints).map(|r| r.to_json())
    })
    .await;

    match result {
        Ok(Ok(v)) => Ok(Json(v)),
        Ok(Err(e)) => Err(err(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string())),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
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

    let device = std::env::var("LAYA_DEVICE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "cpu".to_string());
    let app_state = AppState {
        router: Arc::new(build_router()),
        gate: Arc::new(Semaphore::new(1)),
        api_key: std::env::var("LAYA_API_KEY").ok().filter(|s| !s.is_empty()),
        device,
    };

    let app = AxumRouter::new()
        .route("/health", get(health))
        .route("/v1/systemone", post(systemone))
        .with_state(app_state);

    let host = std::env::var("LAYA_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("LAYA_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8000);
    let addr: SocketAddr = format!("{host}:{port}").parse().expect("valid bind address");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("laya-serve listening on http://{addr}");
    axum::serve(listener, app).await.expect("server");
}
