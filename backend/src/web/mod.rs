//! Serves the built SPA. Unknown paths get `index.html`, so client-side routes work on
//! reload, except under the server's own prefixes, which must stay plain 404s.

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::app_state::AppState;

const SERVER_PREFIXES: [&str; 4] = ["/api/", "/oauth/", "/mcp/", "/.well-known/"];

pub fn routes(static_dir: &Path) -> Router<AppState> {
    let files = ServeDir::new(static_dir).fallback(ServeFile::new(static_dir.join("index.html")));
    Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn(reject_server_paths))
        // Nobody may frame these pages: it would allow clickjacking the consent screen.
        .layer(security_header(
            header::CONTENT_SECURITY_POLICY,
            "frame-ancestors 'none'",
        ))
        .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(security_header(header::REFERRER_POLICY, "no-referrer"))
}

fn security_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

async fn reject_server_paths(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if SERVER_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}
