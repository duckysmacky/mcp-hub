use sqlx::PgPool;

use crate::{crypto::CryptoError, error::ApiError};

pub struct ActiveSession {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub wrapped_dek: Vec<u8>,
}

/// Creates a session. The DEK wrap is bound to the row id, so `wrap` runs once it's known.
pub async fn create(
    pool: &PgPool,
    user_id: i64,
    token_hash: &[u8],
    ttl_secs: u64,
    wrap: impl FnOnce(i64) -> Result<Vec<u8>, CryptoError>,
) -> Result<i64, ApiError> {
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO web_sessions (token_hash, user_id, wrapped_dek, expires_at)
         VALUES ($1, $2, ''::bytea, now() + make_interval(secs => $3))
         RETURNING id",
        token_hash,
        user_id,
        ttl_secs as f64,
    )
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query!(
        "UPDATE web_sessions SET wrapped_dek = $1 WHERE id = $2",
        wrap(id)?,
        id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(id)
}

/// Finds an unexpired session and slides its expiry forward.
pub async fn find_active(
    pool: &PgPool,
    token_hash: &[u8],
    ttl_secs: u64,
) -> Result<Option<ActiveSession>, sqlx::Error> {
    sqlx::query_as!(
        ActiveSession,
        "UPDATE web_sessions AS session
         SET last_seen_at = now(), expires_at = now() + make_interval(secs => $2)
         FROM users
         WHERE session.token_hash = $1
           AND session.expires_at > now()
           AND users.id = session.user_id
         RETURNING session.id, session.user_id, users.username, session.wrapped_dek",
        token_hash,
        ttl_secs as f64,
    )
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM web_sessions WHERE id = $1", id)
        .execute(pool)
        .await?;
    Ok(())
}
