use sqlx::{PgPool, types::Json};
use time::OffsetDateTime;

use crate::{
    crypto::{CryptoError, KdfParams},
    error::{ApiError, conflict_on_duplicate},
};

pub const INVALID_INVITE: &str = "this invite code is not valid";

pub struct NewUser<'a> {
    pub username: &'a str,
    pub password_hash: &'a str,
    pub kek_salt: &'a [u8],
    pub kek_params: KdfParams,
    /// Consumed in the same transaction, so that a code can create only one account.
    pub invite_hash: Option<&'a [u8]>,
}

pub struct UserCredentials {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub kek_salt: Vec<u8>,
    pub kek_params: Json<KdfParams>,
    pub wrapped_dek: Vec<u8>,
    pub is_disabled: bool,
}

pub struct UserSummary {
    pub id: i64,
    pub username: String,
    pub created_at: OffsetDateTime,
    pub disabled_at: Option<OffsetDateTime>,
}

/// Creates a user. The DEK wrap is bound to the row id, so `wrap` runs once it's known.
pub async fn create(
    pool: &PgPool,
    user: NewUser<'_>,
    wrap: impl FnOnce(i64) -> Result<Vec<u8>, CryptoError>,
) -> Result<i64, ApiError> {
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO users (username, password_hash, kek_salt, kek_params, wrapped_dek)
         VALUES ($1, $2, $3, $4, ''::bytea)
         RETURNING id",
        user.username,
        user.password_hash,
        user.kek_salt,
        Json(user.kek_params) as _,
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| conflict_on_duplicate(error, "username is taken"))?;

    if let Some(invite_hash) = user.invite_hash {
        let claimed = sqlx::query!(
            "UPDATE invites SET used_at = now(), used_by = $2
             WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()",
            invite_hash,
            id,
        )
        .execute(&mut *transaction)
        .await?;
        if claimed.rows_affected() == 0 {
            return Err(ApiError::Forbidden(INVALID_INVITE));
        }
    }

    sqlx::query!(
        "UPDATE users SET wrapped_dek = $1 WHERE id = $2",
        wrap(id)?,
        id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(id)
}

pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Option<UserCredentials>, sqlx::Error> {
    sqlx::query_as!(
        UserCredentials,
        r#"SELECT id, username, password_hash, kek_salt,
                  kek_params AS "kek_params: Json<KdfParams>", wrapped_dek,
                  disabled_at IS NOT NULL AS "is_disabled!"
           FROM users WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
}

pub struct NewPassword<'a> {
    pub password_hash: &'a str,
    pub kek_salt: &'a [u8],
    pub kek_params: KdfParams,
    pub wrapped_dek: &'a [u8],
}

/// Stores a new password and logs out every web session except the one making the
/// change. OAuth grants keep working, because the DEK they wrap hasn't changed.
pub async fn change_password(
    pool: &PgPool,
    user_id: i64,
    keep_session_id: i64,
    password: NewPassword<'_>,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "UPDATE users SET password_hash = $2, kek_salt = $3, kek_params = $4, wrapped_dek = $5
         WHERE id = $1",
        user_id,
        password.password_hash,
        password.kek_salt,
        Json(password.kek_params) as _,
        password.wrapped_dek,
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query!(
        "DELETE FROM web_sessions WHERE user_id = $1 AND id <> $2",
        user_id,
        keep_session_id,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

pub async fn find_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<UserCredentials>, sqlx::Error> {
    sqlx::query_as!(
        UserCredentials,
        r#"SELECT id, username, password_hash, kek_salt,
                  kek_params AS "kek_params: Json<KdfParams>", wrapped_dek,
                  disabled_at IS NOT NULL AS "is_disabled!"
           FROM users WHERE lower(username) = lower($1)"#,
        username,
    )
    .fetch_optional(pool)
    .await
}

pub async fn list(pool: &PgPool) -> Result<Vec<UserSummary>, sqlx::Error> {
    sqlx::query_as!(
        UserSummary,
        "SELECT id, username, created_at, disabled_at FROM users ORDER BY id"
    )
    .fetch_all(pool)
    .await
}

/// Locks a user out: their sessions and grants go, along with every DEK wrap in them.
pub async fn disable(pool: &PgPool, user_id: i64) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "UPDATE users SET disabled_at = now() WHERE id = $1 AND disabled_at IS NULL",
        user_id,
    )
    .execute(&mut *transaction)
    .await?;
    delete_access(&mut transaction, user_id).await?;
    transaction.commit().await
}

/// Lets a user log in again. Their old grants stay gone.
pub async fn enable(pool: &PgPool, user_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE users SET disabled_at = NULL WHERE id = $1", user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// An operator's password reset. The old DEK can't be recovered, so everything encrypted
/// under it and everything that wraps it is deleted (SECRETS.md).
pub async fn reset_password(
    pool: &PgPool,
    user_id: i64,
    password: NewPassword<'_>,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "UPDATE users SET password_hash = $2, kek_salt = $3, kek_params = $4, wrapped_dek = $5
         WHERE id = $1",
        user_id,
        password.password_hash,
        password.kek_salt,
        Json(password.kek_params) as _,
        password.wrapped_dek,
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query!(
        "DELETE FROM instance_env
         WHERE instance_id IN (SELECT id FROM instances WHERE user_id = $1)",
        user_id,
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query!(
        "UPDATE instances
         SET enabled = false, needs_attention = 'secrets_reset', updated_at = now()
         WHERE user_id = $1",
        user_id,
    )
    .execute(&mut *transaction)
    .await?;
    delete_access(&mut transaction, user_id).await?;
    transaction.commit().await
}

async fn delete_access(
    connection: &mut sqlx::PgConnection,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM web_sessions WHERE user_id = $1", user_id)
        .execute(&mut *connection)
        .await?;
    // Tokens and codes cascade from their grant.
    sqlx::query!("DELETE FROM oauth_grants WHERE user_id = $1", user_id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}
