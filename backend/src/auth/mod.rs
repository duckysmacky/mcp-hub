//! Accounts: sign-up, password login, web sessions (SECRETS.md lifecycles).

use axum::{
    Json, RequestPartsExt, Router,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use zeroize::Zeroizing;

use crate::{
    app_state::AppState,
    crypto::{self, Dek, KdfParams, Token, WrapKind},
    db,
    error::ApiError,
};

const SESSION_COOKIE: &str = "mcphub_session";
const MAX_USERNAME_CHARS: usize = 64;
const MIN_PASSWORD_CHARS: usize = 8;
// Bounds the work Argon2 does on attacker-supplied input.
const MAX_PASSWORD_BYTES: usize = 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/config", get(config))
        .route("/signup", post(signup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
}

/// The logged-in user behind a request. Rejects with 401 when there's no valid session.
pub struct CurrentSession {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    wrapped_dek: Vec<u8>,
    token: Token,
}

impl CurrentSession {
    /// Unwraps the DEK for the moment it's needed. Drop it straight after.
    pub fn dek(&self) -> Result<Dek, ApiError> {
        let kek = self.token.kek(WrapKind::WebSession)?;
        Ok(crypto::unwrap_dek(
            &kek,
            &self.wrapped_dek,
            WrapKind::WebSession,
            self.id,
            self.user_id,
        )?)
    }
}

impl FromRequestParts<AppState> for CurrentSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let headers: HeaderMap = parts.extract().await.unwrap_or_default();
        let token = session_cookie(&headers)
            .and_then(|value| Token::parse("", value))
            .ok_or(ApiError::Unauthorized)?;
        let ttl = state.config.settings.sessions.web_session_ttl_secs;
        let session = db::web_sessions::find_active(&state.pool, &token.hash(), ttl)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        Ok(Self {
            id: session.id,
            user_id: session.user_id,
            username: session.username,
            wrapped_dek: session.wrapped_dek,
            token,
        })
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
}

#[derive(Serialize)]
struct AuthConfig {
    signup_enabled: bool,
}

async fn config(State(state): State<AppState>) -> Json<AuthConfig> {
    Json(AuthConfig {
        signup_enabled: state.config.allow_signup,
    })
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: Zeroizing<String>,
}

#[derive(Serialize)]
struct UserResponse {
    id: i64,
    username: String,
}

fn validate_new_credentials(credentials: &Credentials) -> Result<(), ApiError> {
    let username = &credentials.username;
    let username_is_valid = (1..=MAX_USERNAME_CHARS).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !username_is_valid {
        return Err(ApiError::BadRequest(format!(
            "username must be 1-{MAX_USERNAME_CHARS} letters, digits, or - _ ."
        )));
    }
    if credentials.password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(ApiError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    if credentials.password.len() > MAX_PASSWORD_BYTES {
        return Err(ApiError::BadRequest("password is too long".to_owned()));
    }
    Ok(())
}

async fn signup(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<impl IntoResponse, ApiError> {
    if !state.config.allow_signup {
        return Err(ApiError::Forbidden("sign-up is disabled"));
    }
    validate_new_credentials(&credentials)?;

    let params = state.config.settings.password_kdf.params();
    let kek_salt = crypto::generate_salt();
    let (password_hash, kek) = {
        let (password, salt) = (credentials.password, kek_salt.clone());
        spawn_blocking(move || {
            Ok::<_, crypto::CryptoError>((
                crypto::hash_password(&password, params)?,
                crypto::derive_password_kek(&password, &salt, params)?,
            ))
        })
        .await??
    };

    let dek = Dek::generate();
    let id = db::users::create(
        &state.pool,
        db::users::NewUser {
            username: &credentials.username,
            password_hash: &password_hash,
            kek_salt: &kek_salt,
            kek_params: params,
        },
        |user_id| crypto::wrap_dek(&kek, &dek, WrapKind::Password, user_id, user_id),
    )
    .await?;

    let user = UserResponse {
        id,
        username: credentials.username,
    };
    Ok((StatusCode::CREATED, Json(user)))
}

async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<impl IntoResponse, ApiError> {
    if credentials.password.len() > MAX_PASSWORD_BYTES {
        return Err(ApiError::InvalidCredentials);
    }
    let Some(user) = db::users::find_by_username(&state.pool, &credentials.username).await? else {
        // Spend the same time as a real check, so usernames can't be probed by timing.
        let params = state.config.settings.password_kdf.params();
        spawn_blocking(move || crypto::hash_password(&credentials.password, params)).await??;
        return Err(ApiError::InvalidCredentials);
    };

    let dek = unwrap_with_password(
        credentials.password,
        user.password_hash,
        user.kek_salt,
        user.kek_params.0,
        user.wrapped_dek,
        user.id,
    )
    .await?;

    let token = Token::generate();
    let session_kek = token.kek(WrapKind::WebSession)?;
    let ttl = state.config.settings.sessions.web_session_ttl_secs;
    db::web_sessions::create(&state.pool, user.id, &token.hash(), ttl, |session_id| {
        crypto::wrap_dek(
            &session_kek,
            &dek,
            WrapKind::WebSession,
            session_id,
            user.id,
        )
    })
    .await?;

    let cookie = session_cookie_header(&state, &token.encode(""), ttl);
    let body = UserResponse {
        id: user.id,
        username: user.username,
    };
    Ok(([(header::SET_COOKIE, cookie)], Json(body)))
}

async fn unwrap_with_password(
    password: Zeroizing<String>,
    password_hash: String,
    kek_salt: Vec<u8>,
    kek_params: KdfParams,
    wrapped_dek: Vec<u8>,
    user_id: i64,
) -> Result<Dek, ApiError> {
    spawn_blocking(move || {
        if !crypto::verify_password(&password, &password_hash) {
            return Err(ApiError::InvalidCredentials);
        }
        let kek = crypto::derive_password_kek(&password, &kek_salt, kek_params)?;
        Ok(crypto::unwrap_dek(
            &kek,
            &wrapped_dek,
            WrapKind::Password,
            user_id,
            user_id,
        )?)
    })
    .await?
}

fn session_cookie_header(state: &AppState, value: &str, max_age_secs: u64) -> String {
    let secure = if state.config.public_url.is_https() {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_secs}{secure}"
    )
}

async fn logout(
    State(state): State<AppState>,
    session: CurrentSession,
) -> Result<impl IntoResponse, ApiError> {
    db::web_sessions::delete(&state.pool, session.id).await?;
    let cookie = session_cookie_header(&state, "", 0);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}

async fn me(session: CurrentSession) -> Json<UserResponse> {
    Json(UserResponse {
        id: session.user_id,
        username: session.username,
    })
}
