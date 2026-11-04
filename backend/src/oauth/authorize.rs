//! `GET /oauth/authorize`: validates the request, then hands over to the consent page.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use url::Url;

use super::{SCOPE, slug_from_resource};
use crate::{
    app_state::AppState,
    config::host_is_loopback,
    crypto::Token,
    db,
    db::oauth::{Client, NewPending},
    error::ApiError,
};

const PENDING_TTL_SECS: u64 = 600;
const CHALLENGE_LENGTHS: std::ops::RangeInclusive<usize> = 43..=128;

#[derive(Deserialize)]
pub struct AuthorizeParams {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
}

pub async fn authorize(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response, ApiError> {
    // With a bad client or redirect target, nothing may be sent to that target.
    let Some(client_id) = params.client_id.as_deref() else {
        return Ok(error_page("client_id is missing"));
    };
    let Some(client) = db::oauth::find_client(&state.pool, client_id).await? else {
        return Ok(error_page("unknown client_id"));
    };
    let Some(redirect_uri) = params.redirect_uri.as_deref() else {
        return Ok(error_page("redirect_uri is missing"));
    };
    if !redirect_uri_matches(&client, redirect_uri) {
        return Ok(error_page("redirect_uri is not registered for this client"));
    }

    let redirect_error = |code: &str| {
        let location = error_redirect(
            redirect_uri,
            code,
            params.state.as_deref(),
            state.config.public_url.as_str(),
        );
        Ok(Redirect::to(&location).into_response())
    };

    let challenge = params.code_challenge.as_deref().unwrap_or_default();
    if params.response_type.as_deref() != Some("code")
        || params.code_challenge_method.as_deref() != Some("S256")
        || !CHALLENGE_LENGTHS.contains(&challenge.len())
    {
        return redirect_error("invalid_request");
    }
    let resource = params.resource.as_deref().unwrap_or_default();
    if slug_from_resource(&state.config.public_url, resource).is_none() {
        return redirect_error("invalid_target");
    }
    if params.scope.as_deref().is_some_and(|scope| scope != SCOPE) {
        return redirect_error("invalid_scope");
    }

    let pending_id = Token::generate().encode("");
    db::oauth::create_pending(
        &state.pool,
        NewPending {
            id: &pending_id,
            client_id,
            redirect_uri,
            code_challenge: challenge,
            state: params.state.as_deref(),
            scope: SCOPE,
            resource,
            ttl_secs: PENDING_TTL_SECS,
        },
    )
    .await?;
    Ok(Redirect::to(&format!("/consent?request={}", pending_id.as_str())).into_response())
}

fn error_page(message: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, message).into_response()
}

/// Exact match, except that a loopback redirect may use any port (RFC 8252 §7.3),
/// because native apps pick a free port when they start.
fn redirect_uri_matches(client: &Client, redirect_uri: &str) -> bool {
    client.redirect_uris.iter().any(|registered| {
        registered == redirect_uri || same_ignoring_loopback_port(registered, redirect_uri)
    })
}

fn same_ignoring_loopback_port(registered: &str, requested: &str) -> bool {
    let (Ok(registered), Ok(requested)) = (Url::parse(registered), Url::parse(requested)) else {
        return false;
    };
    host_is_loopback(&registered)
        && registered.scheme() == requested.scheme()
        && registered.host() == requested.host()
        && registered.path() == requested.path()
        && registered.query() == requested.query()
        && requested.fragment().is_none()
        && requested.username().is_empty()
        && requested.password().is_none()
}

/// Builds the redirect that tells a client how authorization ended.
pub fn error_redirect(
    redirect_uri: &str,
    error: &str,
    state: Option<&str>,
    issuer: &str,
) -> String {
    redirect_with(redirect_uri, &[("error", error)], state, issuer)
}

pub fn redirect_with(
    redirect_uri: &str,
    params: &[(&str, &str)],
    state: Option<&str>,
    issuer: &str,
) -> String {
    let mut url = Url::parse(redirect_uri).expect("redirect URIs are validated at registration");
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in params {
            query.append_pair(name, value);
        }
        if let Some(state) = state {
            query.append_pair("state", state);
        }
        query.append_pair("iss", issuer);
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(uris: &[&str]) -> Client {
        Client {
            client_id: "client".to_owned(),
            client_name: None,
            redirect_uris: sqlx::types::Json(uris.iter().map(|uri| (*uri).to_owned()).collect()),
        }
    }

    #[test]
    fn https_redirects_must_match_exactly() {
        let client = client(&["https://app.example/callback"]);
        assert!(redirect_uri_matches(
            &client,
            "https://app.example/callback"
        ));
        for other in [
            "https://app.example/callback/",
            "https://app.example/other",
            "https://app.example:8443/callback",
            "https://app.example/callback?x=1",
            "https://evil.example/callback",
            "http://app.example/callback",
        ] {
            assert!(!redirect_uri_matches(&client, other), "{other}");
        }
    }

    #[test]
    fn loopback_redirects_may_change_only_the_port() {
        let client = client(&["http://127.0.0.1:1000/callback", "http://localhost/cb"]);
        assert!(redirect_uri_matches(
            &client,
            "http://127.0.0.1:2000/callback"
        ));
        assert!(redirect_uri_matches(&client, "http://localhost:33333/cb"));
        for other in [
            "http://127.0.0.1:2000/other",
            "http://[::1]:2000/callback",
            "https://127.0.0.1:2000/callback",
            "http://evil.example:2000/callback",
            "http://user@127.0.0.1:2000/callback",
            "http://127.0.0.1:2000/callback#fragment",
        ] {
            assert!(!redirect_uri_matches(&client, other), "{other}");
        }
    }

    #[test]
    fn redirect_keeps_existing_query_and_encodes_values() {
        let location = redirect_with(
            "https://app.example/cb?keep=1",
            &[("code", "a b")],
            Some("s&t"),
            "https://hub.example",
        );
        assert_eq!(
            location,
            "https://app.example/cb?keep=1&code=a+b&state=s%26t&iss=https%3A%2F%2Fhub.example"
        );
    }
}
