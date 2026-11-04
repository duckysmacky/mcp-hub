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
