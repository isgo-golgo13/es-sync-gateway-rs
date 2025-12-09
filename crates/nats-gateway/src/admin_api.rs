//! Admin API for NATS Gateway

use axum::{extract::State, routing::get, Json, Router};
use es_gateway_core::filter::FilterChain;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::GatewayStats;

/// Admin API state
#[derive(Clone)]
pub struct AdminState {
    pub filter_chain: Arc<RwLock<FilterChain>>,
    pub stats: Arc<dyn Fn() -> GatewayStats + Send + Sync>,
}

/// Health response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

/// Stats response
#[derive(Serialize)]
pub struct StatsResponse {
    pub received: u64,
    pub filtered: u64,
    pub forwarded: u64,
    pub filter_rate: f64,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn stats(State(state): State<AdminState>) -> Json<StatsResponse> {
    let s = (state.stats)();
    let filter_rate = if s.received > 0 {
        s.filtered as f64 / s.received as f64 * 100.0
    } else {
        0.0
    };
    Json(StatsResponse {
        received: s.received,
        filtered: s.filtered,
        forwarded: s.forwarded,
        filter_rate,
    })
}

async fn ready() -> &'static str {
    "OK"
}

/// Run admin server
pub async fn run_admin_server(listen: String, state: AdminState) {
    let app = Router::new()
        .route("/health", get(health))
        .route("/health/live", get(ready))
        .route("/health/ready", get(ready))
        .route("/stats", get(stats))
        .route("/metrics", get(|| async { "# TODO: Prometheus metrics" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen).await.unwrap();
    info!(listen, "Admin API started");
    axum::serve(listener, app).await.unwrap();
}
