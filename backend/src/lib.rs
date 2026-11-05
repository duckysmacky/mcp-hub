pub mod api;
pub mod app_state;
pub mod audit;
pub mod auth;
pub mod bridge;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod mcp;
pub mod net_guard;
pub mod oauth;
pub mod process;
pub mod supervisor;
pub mod web;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

use app_state::AppState;

pub fn build_router(state: AppState) -> Router {
    let max_body_bytes = state.config.settings.limits.max_body_bytes;
    Router::new()
        .route("/health", get(health))
        .nest("/api", api::routes(state.clone()))
        .merge(oauth::routes())
        .merge(mcp::routes())
        .merge(web::routes(&state.config.settings.paths.r#static))
        .layer(DefaultBodyLimit::max(max_body_bytes))
        // Only the path: query strings can carry OAuth parameters.
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request| {
            tracing::info_span!("request", method = %request.method(), path = request.uri().path())
        }))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ok" }))),
        Err(error) => {
            tracing::error!(%error, "health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "database unreachable" })),
            )
        }
    }
}
