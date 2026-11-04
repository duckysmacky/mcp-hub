use sqlx::{PgPool, types::Json};

use crate::{crypto::CryptoError, error::ApiError};

pub struct Client {
    pub client_id: String,
    pub client_name: Option<String>,
    pub redirect_uris: Json<Vec<String>>,
}

pub struct NewPending<'a> {
    pub id: &'a str,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub code_challenge: &'a str,
    pub state: Option<&'a str>,
    pub scope: &'a str,
    pub resource: &'a str,
    pub ttl_secs: u64,
}

pub struct Pending {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: Option<String>,
    pub scope: String,
    pub resource: String,
}

pub struct NewCode<'a> {
    pub code_hash: &'a [u8],
    pub grant_id: i64,
    pub redirect_uri: &'a str,
    pub code_challenge: &'a str,
    pub resource: &'a str,
    pub ttl_secs: u64,
}

pub async fn create_client(
    pool: &PgPool,
    client_id: &str,
    client_name: Option<&str>,
    redirect_uris: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ($1, $2, $3)",
        client_id,
        client_name,
        Json(redirect_uris) as _,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_client(pool: &PgPool, client_id: &str) -> Result<Option<Client>, sqlx::Error> {
    sqlx::query_as!(
        Client,
        r#"SELECT client_id, client_name, redirect_uris AS "redirect_uris: Json<Vec<String>>"
           FROM oauth_clients WHERE client_id = $1"#,
        client_id,
    )
    .fetch_optional(pool)
    .await
}

pub async fn create_pending(pool: &PgPool, pending: NewPending<'_>) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO oauth_pending_authorizations
             (id, client_id, redirect_uri, code_challenge, state, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(secs => $8))",
        pending.id,
        pending.client_id,
        pending.redirect_uri,
        pending.code_challenge,
        pending.state,
        pending.scope,
        pending.resource,
        pending.ttl_secs as f64,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Reads a pending authorization that can still be decided.
pub async fn find_pending(pool: &PgPool, id: &str) -> Result<Option<Pending>, sqlx::Error> {
    sqlx::query_as!(
        Pending,
        "SELECT client_id, redirect_uri, code_challenge, state, scope, resource
         FROM oauth_pending_authorizations
         WHERE id = $1 AND consumed_at IS NULL AND expires_at > now()",
        id,
    )
    .fetch_optional(pool)
    .await
}

/// Claims a pending authorization atomically, so it can be decided only once.
pub async fn consume_pending(pool: &PgPool, id: &str) -> Result<Option<Pending>, sqlx::Error> {
    sqlx::query_as!(
        Pending,
        "UPDATE oauth_pending_authorizations SET consumed_at = now()
         WHERE id = $1 AND consumed_at IS NULL AND expires_at > now()
         RETURNING client_id, redirect_uri, code_challenge, state, scope, resource",
        id,
    )
    .fetch_optional(pool)
    .await
}

/// Returns the user's active grant for this client and instance, creating it if needed.
pub async fn find_or_create_grant(
    pool: &PgPool,
    user_id: i64,
    client_id: &str,
    instance_id: i64,
    resource: &str,
    scope: &str,
) -> Result<i64, sqlx::Error> {
    let existing = sqlx::query_scalar!(
        "SELECT id FROM oauth_grants
         WHERE user_id = $1 AND client_id = $2 AND instance_id = $3
           AND resource = $4 AND scope = $5 AND revoked_at IS NULL",
        user_id,
        client_id,
        instance_id,
        resource,
        scope,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    sqlx::query_scalar!(
        "INSERT INTO oauth_grants (user_id, client_id, instance_id, resource, scope)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
        user_id,
        client_id,
        instance_id,
        resource,
        scope,
    )
    .fetch_one(pool)
    .await
}

/// Creates an auth code. The DEK wrap is bound to the row id, so `wrap` runs once it's known.
pub async fn create_code(
    pool: &PgPool,
    code: NewCode<'_>,
    wrap: impl FnOnce(i64) -> Result<Vec<u8>, CryptoError>,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO oauth_codes
             (code_hash, grant_id, redirect_uri, code_challenge, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))
         RETURNING id",
        code.code_hash,
        code.grant_id,
        code.redirect_uri,
        code.code_challenge,
        code.resource,
        code.ttl_secs as f64,
    )
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query!(
        "UPDATE oauth_codes SET wrapped_dek = $1 WHERE id = $2",
        wrap(id)?,
        id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

pub struct CodeRecord {
    pub id: i64,
    pub grant_id: i64,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub resource: String,
    pub wrapped_dek: Option<Vec<u8>>,
    pub is_expired: bool,
    pub is_used: bool,
    pub grant_is_revoked: bool,
    pub client_id: String,
    pub user_id: i64,
    pub scope: String,
}

pub struct RefreshRecord {
    pub id: i64,
    pub grant_id: i64,
    pub wrapped_dek: Vec<u8>,
    pub is_expired: bool,
    pub is_used: bool,
    pub is_revoked: bool,
    pub grant_is_revoked: bool,
    pub client_id: String,
    pub user_id: i64,
    pub resource: String,
    pub scope: String,
}

/// What a bearer token grants. The caller still has to check the audience.
pub struct AccessRecord {
    pub id: i64,
    pub user_id: i64,
    pub instance_id: i64,
    pub resource: String,
    pub wrapped_dek: Vec<u8>,
}

pub async fn find_code(pool: &PgPool, code_hash: &[u8]) -> Result<Option<CodeRecord>, sqlx::Error> {
    sqlx::query_as!(
        CodeRecord,
        r#"SELECT code.id, code.grant_id, code.redirect_uri, code.code_challenge, code.resource,
                  code.wrapped_dek,
                  code.expires_at <= now() AS "is_expired!",
                  code.used_at IS NOT NULL AS "is_used!",
                  grants.revoked_at IS NOT NULL AS "grant_is_revoked!",
                  grants.client_id, grants.user_id, grants.scope
           FROM oauth_codes AS code
           JOIN oauth_grants AS grants ON grants.id = code.grant_id
           WHERE code.code_hash = $1"#,
        code_hash,
    )
    .fetch_optional(pool)
    .await
}

/// Claims a code and clears its wrap. Returns false if something else claimed it first.
pub async fn claim_code(
    connection: &mut sqlx::PgConnection,
    code_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE oauth_codes SET used_at = now(), wrapped_dek = NULL
         WHERE id = $1 AND used_at IS NULL",
        code_id,
    )
    .execute(connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Code replay: everything issued from that code stops working.
pub async fn revoke_tokens_from_code(pool: &PgPool, code_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE oauth_tokens SET revoked_at = now()
         WHERE issued_from_code = $1 AND revoked_at IS NULL",
        code_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_refresh_token(
    pool: &PgPool,
    token_hash: &[u8],
) -> Result<Option<RefreshRecord>, sqlx::Error> {
    sqlx::query_as!(
        RefreshRecord,
        r#"SELECT token.id, token.grant_id, token.wrapped_dek,
                  token.expires_at <= now() AS "is_expired!",
                  token.used_at IS NOT NULL AS "is_used!",
                  token.revoked_at IS NOT NULL AS "is_revoked!",
                  grants.revoked_at IS NOT NULL AS "grant_is_revoked!",
                  grants.client_id, grants.user_id, grants.resource, grants.scope
           FROM oauth_tokens AS token
           JOIN oauth_grants AS grants ON grants.id = token.grant_id
           WHERE token.token_hash = $1 AND token.kind = 'refresh'"#,
        token_hash,
    )
    .fetch_optional(pool)
    .await
}

/// Marks a refresh token as rotated. Returns false if it already was.
pub async fn claim_refresh_token(
    connection: &mut sqlx::PgConnection,
    token_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE oauth_tokens SET used_at = now() WHERE id = $1 AND used_at IS NULL",
        token_id,
    )
    .execute(connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Refresh token reuse: the whole grant and all its tokens stop working.
pub async fn revoke_grant(pool: &PgPool, grant_id: i64) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "UPDATE oauth_grants SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        grant_id,
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query!(
        "UPDATE oauth_tokens SET revoked_at = now() WHERE grant_id = $1 AND revoked_at IS NULL",
        grant_id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

pub struct NewToken<'a> {
    pub token_hash: &'a [u8],
    pub kind: &'a str,
    pub grant_id: i64,
    pub issued_from_code: Option<i64>,
    pub rotated_from: Option<i64>,
    pub ttl_secs: u64,
}

/// Stores a token. The DEK wrap is bound to the row id, so `wrap` runs once it's known.
pub async fn insert_token(
    connection: &mut sqlx::PgConnection,
    token: NewToken<'_>,
    wrap: impl FnOnce(i64) -> Result<Vec<u8>, CryptoError>,
) -> Result<(), ApiError> {
    let id = sqlx::query_scalar!(
        "INSERT INTO oauth_tokens
             (token_hash, kind, grant_id, issued_from_code, rotated_from, wrapped_dek, expires_at)
         VALUES ($1, $2, $3, $4, $5, ''::bytea, now() + make_interval(secs => $6))
         RETURNING id",
        token.token_hash,
        token.kind,
        token.grant_id,
        token.issued_from_code,
        token.rotated_from,
        token.ttl_secs as f64,
    )
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query!(
        "UPDATE oauth_tokens SET wrapped_dek = $1 WHERE id = $2",
        wrap(id)?,
        id,
    )
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Finds a usable access token: unexpired, unrevoked, and with an active grant.
pub async fn find_access_token(
    pool: &PgPool,
    token_hash: &[u8],
) -> Result<Option<AccessRecord>, sqlx::Error> {
    sqlx::query_as!(
        AccessRecord,
        "SELECT token.id, grants.user_id, grants.instance_id, grants.resource, token.wrapped_dek
         FROM oauth_tokens AS token
         JOIN oauth_grants AS grants ON grants.id = token.grant_id
         WHERE token.token_hash = $1
           AND token.kind = 'access'
           AND token.expires_at > now()
           AND token.revoked_at IS NULL
           AND grants.revoked_at IS NULL",
        token_hash,
    )
    .fetch_optional(pool)
    .await
}
