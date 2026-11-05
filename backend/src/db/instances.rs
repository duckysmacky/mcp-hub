//! Every query that takes a request's id also takes the user id, so ownership can't
//! be forgotten: another user's instance simply isn't found.

use sqlx::PgPool;
use time::OffsetDateTime;

use crate::error::{ApiError, conflict_on_duplicate};

const SLUG_TAKEN: &str = "you already have an instance with this slug";

pub struct Instance {
    pub id: i64,
    pub user_id: i64,
    pub slug: String,
    pub server_id: String,
    pub enabled: bool,
    pub needs_attention: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct EnvEntry {
    pub env_var: String,
    pub updated_at: OffsetDateTime,
}

pub struct EnvCiphertext {
    pub env_var: String,
    pub ciphertext: Vec<u8>,
}

/// Checks `^[a-z0-9][a-z0-9-]{0,62}$`, which the database also enforces.
pub fn is_valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    slug.len() <= 63
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '-')
}

pub async fn create(
    pool: &PgPool,
    user_id: i64,
    slug: &str,
    server_id: &str,
) -> Result<Instance, ApiError> {
    sqlx::query_as!(
        Instance,
        "INSERT INTO instances (user_id, slug, server_id)
         VALUES ($1, $2, $3)
         RETURNING id, user_id, slug, server_id, enabled, needs_attention,
                   created_at, updated_at",
        user_id,
        slug,
        server_id,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| conflict_on_duplicate(error, SLUG_TAKEN))
}

pub async fn list(pool: &PgPool, user_id: i64) -> Result<Vec<Instance>, sqlx::Error> {
    sqlx::query_as!(
        Instance,
        "SELECT id, user_id, slug, server_id, enabled, needs_attention,
                created_at, updated_at
         FROM instances WHERE user_id = $1 ORDER BY slug",
        user_id,
    )
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &PgPool, user_id: i64, id: i64) -> Result<Option<Instance>, sqlx::Error> {
    sqlx::query_as!(
        Instance,
        "SELECT id, user_id, slug, server_id, enabled, needs_attention,
                created_at, updated_at
         FROM instances WHERE id = $1 AND user_id = $2",
        id,
        user_id,
    )
    .fetch_optional(pool)
    .await
}

pub async fn find_by_slug(
    pool: &PgPool,
    user_id: i64,
    slug: &str,
) -> Result<Option<Instance>, sqlx::Error> {
    sqlx::query_as!(
        Instance,
        "SELECT id, user_id, slug, server_id, enabled, needs_attention,
                created_at, updated_at
         FROM instances WHERE user_id = $1 AND slug = $2",
        user_id,
        slug,
    )
    .fetch_optional(pool)
    .await
}

/// Changes the given fields; `None` leaves a field as it is.
pub async fn update(
    pool: &PgPool,
    user_id: i64,
    id: i64,
    slug: Option<&str>,
) -> Result<Option<Instance>, ApiError> {
    sqlx::query_as!(
        Instance,
        "UPDATE instances
         SET slug = COALESCE($3, slug), updated_at = now()
         WHERE id = $1 AND user_id = $2
         RETURNING id, user_id, slug, server_id, enabled, needs_attention,
                   created_at, updated_at",
        id,
        user_id,
        slug,
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| conflict_on_duplicate(error, SLUG_TAKEN))
}

pub async fn delete(pool: &PgPool, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM instances WHERE id = $1 AND user_id = $2",
        id,
        user_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_enabled(
    pool: &PgPool,
    user_id: i64,
    id: i64,
    enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE instances SET enabled = $3, updated_at = now() WHERE id = $1 AND user_id = $2",
        id,
        user_id,
        enabled,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Flags instances whose server left the catalog or was turned off, and clears the flag
/// if it came back. A returning server stays disabled until its owner starts it again.
pub async fn sync_with_catalog(
    pool: &PgPool,
    enabled_server_ids: &[String],
    disabled_server_ids: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE instances
         SET enabled = false, needs_attention = 'server_removed', updated_at = now()
         WHERE NOT (server_id = ANY($1) OR server_id = ANY($2))
           AND needs_attention IS DISTINCT FROM 'server_removed'",
        enabled_server_ids,
        disabled_server_ids,
    )
    .execute(pool)
    .await?;
    sqlx::query!(
        "UPDATE instances
         SET enabled = false, needs_attention = 'server_disabled', updated_at = now()
         WHERE server_id = ANY($1) AND needs_attention IS DISTINCT FROM 'server_disabled'",
        disabled_server_ids,
    )
    .execute(pool)
    .await?;
    sqlx::query!(
        "UPDATE instances SET needs_attention = NULL, updated_at = now()
         WHERE server_id = ANY($1) AND needs_attention IN ('server_removed', 'server_disabled')",
        enabled_server_ids,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Clears the flag left by a password reset, once the owner starts the instance again.
pub async fn clear_secrets_reset(pool: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE instances SET needs_attention = NULL
         WHERE id = $1 AND needs_attention = 'secrets_reset'",
        id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The ids of every instance of a catalog server, whoever owns them.
pub async fn ids_of_server(pool: &PgPool, server_id: &str) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT id FROM instances WHERE server_id = $1", server_id)
        .fetch_all(pool)
        .await
}

/// The caller must have checked that the instance belongs to the user.
pub async fn set_env(
    pool: &PgPool,
    instance_id: i64,
    env_var: &str,
    ciphertext: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO instance_env (instance_id, env_var, ciphertext)
         VALUES ($1, $2, $3)
         ON CONFLICT (instance_id, env_var)
         DO UPDATE SET ciphertext = EXCLUDED.ciphertext, updated_at = now()",
        instance_id,
        env_var,
        ciphertext,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The caller must have checked that the instance belongs to the user.
pub async fn delete_env(
    pool: &PgPool,
    instance_id: i64,
    env_var: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM instance_env WHERE instance_id = $1 AND env_var = $2",
        instance_id,
        env_var,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Names and dates only: nothing the API returns ever touches a ciphertext.
pub async fn list_env(pool: &PgPool, instance_id: i64) -> Result<Vec<EnvEntry>, sqlx::Error> {
    sqlx::query_as!(
        EnvEntry,
        "SELECT env_var, updated_at FROM instance_env WHERE instance_id = $1 ORDER BY env_var",
        instance_id,
    )
    .fetch_all(pool)
    .await
}

pub async fn env_ciphertexts(
    pool: &PgPool,
    instance_id: i64,
) -> Result<Vec<EnvCiphertext>, sqlx::Error> {
    sqlx::query_as!(
        EnvCiphertext,
        "SELECT env_var, ciphertext FROM instance_env WHERE instance_id = $1",
        instance_id,
    )
    .fetch_all(pool)
    .await
}
