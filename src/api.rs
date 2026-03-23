//! HTTP REST API for rust-network-mgr.
//!
//! Exposes the daemon state and control commands over a local HTTP interface,
//! enabling integration with any tool that speaks HTTP — including AI assistants,
//! MCP servers, Ansible, and monitoring systems.
//!
//! Default bind address: `127.0.0.1:9100`
//!
//! ## Endpoints
//!
//! | Method | Path        | Description                              |
//! |--------|-------------|------------------------------------------|
//! | GET    | /health     | Liveness probe (`{"status":"ok"}`)       |
//! | GET    | /status     | Interfaces + containers + version        |
//! | GET    | /interfaces | Current interface→IP mapping             |
//! | GET    | /containers | Docker container→IP mapping              |
//! | POST   | /reload     | Trigger config reload                    |
//! | GET    | /metrics    | Prometheus text format (basic counters)  |

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::types::{EventSender, NetworkState, SystemEvent, ControlCommand};

// ---------------------------------------------------------------------------
// Shared state passed into Axum handlers
// ---------------------------------------------------------------------------

/// State visible to HTTP handlers.  The main daemon populates this via
/// `Arc` clones; Axum handlers only read it (or send events).
#[derive(Clone)]
pub struct ApiState {
    pub network_state: Arc<Mutex<NetworkState>>,
    pub container_ips: Arc<Mutex<HashMap<String, IpAddr>>>,
    pub event_tx: EventSender,
    pub version: &'static str,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct StatusResponse {
    version: &'static str,
    interfaces: HashMap<String, Vec<String>>,
    containers: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn get_status(State(state): State<ApiState>) -> Json<StatusResponse> {
    let ns = state.network_state.lock().await;
    let interfaces: HashMap<String, Vec<String>> = ns
        .interface_ips
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().map(|ip| ip.to_string()).collect()))
        .collect();

    let containers: HashMap<String, String> = state
        .container_ips
        .lock()
        .await
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string()))
        .collect();

    Json(StatusResponse {
        version: state.version,
        interfaces,
        containers,
    })
}

async fn get_interfaces(State(state): State<ApiState>) -> Json<Value> {
    let ns = state.network_state.lock().await;
    let map: HashMap<String, Vec<String>> = ns
        .interface_ips
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().map(|ip| ip.to_string()).collect()))
        .collect();
    Json(json!(map))
}

async fn get_containers(State(state): State<ApiState>) -> Json<Value> {
    let map: HashMap<String, String> = state
        .container_ips
        .lock()
        .await
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string()))
        .collect();
    Json(json!(map))
}

async fn post_reload(State(state): State<ApiState>) -> impl IntoResponse {
    match state
        .event_tx
        .send(SystemEvent::Control(ControlCommand::Reload))
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

/// Minimal Prometheus-compatible text format.
/// For a full implementation add the `prometheus-client` crate.
async fn get_metrics(State(state): State<ApiState>) -> String {
    let ns = state.network_state.lock().await;
    let container_count = state.container_ips.lock().await.len();
    let interface_count = ns.interface_ips.len();
    format!(
        "# HELP network_mgr_interfaces_total Number of monitored interfaces\n\
         # TYPE network_mgr_interfaces_total gauge\n\
         network_mgr_interfaces_total {}\n\
         # HELP network_mgr_containers_total Number of tracked Docker containers\n\
         # TYPE network_mgr_containers_total gauge\n\
         network_mgr_containers_total {}\n",
        interface_count, container_count,
    )
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the Axum router.  Call this once and pass the result to
/// `axum::serve` bound to the desired address.
pub fn build_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(get_status))
        .route("/interfaces", get(get_interfaces))
        .route("/containers", get(get_containers))
        .route("/reload", post(post_reload))
        .route("/metrics", get(get_metrics))
        .with_state(state)
}

/// Spawn the HTTP server as a background Tokio task.
///
/// Returns the `JoinHandle` so the caller can abort it on shutdown.
pub fn spawn_http_server(
    state: ApiState,
    bind_addr: &str,
) -> tokio::task::JoinHandle<()> {
    let bind_addr = bind_addr.to_string();
    tokio::spawn(async move {
        let router = build_router(state);
        let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("HTTP API: failed to bind {}: {}", bind_addr, e);
                return;
            }
        };
        log::info!("HTTP API listening on http://{}", bind_addr);
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("HTTP API server error: {}", e);
        }
    })
}
