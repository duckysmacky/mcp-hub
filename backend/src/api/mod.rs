//! The JSON API for the web UI, under `/api`.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{app_state::AppState, auth};

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .nest("/auth", auth::routes())
        .layer(middleware::from_fn_with_state(state, require_same_origin))
}

/// CSRF protection (SECURITY.md T10): a state-changing call must come from our own
/// origin, and a form post can't send a JSON content type without a CORS preflight.
async fn require_same_origin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method();
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS {
        return next.run(request).await;
    }

    let headers = request.headers();
    let origin_matches = headers
        .get(header::ORIGIN)
        .is_some_and(|origin| origin.as_bytes() == state.config.public_url.as_str().as_bytes());
    if !origin_matches {
        return forbidden("Origin header missing or not allowed");
    }

    let needs_json = method != Method::DELETE;
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        });
    if needs_json && !is_json {
        return forbidden("Content-Type must be application/json");
    }

    next.run(request).await
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": message }))).into_response()
}
