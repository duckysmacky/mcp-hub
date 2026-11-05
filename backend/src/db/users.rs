use sqlx::{PgPool, types::Json};

use crate::{
    crypto::{CryptoError, KdfParams},
    error::{ApiError, conflict_on_duplicate},
};

pub struct NewUser<'a> {
    pub username: &'a str,
    pub password_hash: &'a str,
    pub kek_salt: &'a [u8],
    pub kek_params: KdfParams,
}

pub struct UserCredentials {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub kek_salt: Vec<u8>,
    pub kek_params: Json<KdfParams>,
    pub wrapped_dek: Vec<u8>,
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
                  kek_params AS "kek_params: Json<KdfParams>", wrapped_dek
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
                  kek_params AS "kek_params: Json<KdfParams>", wrapped_dek
           FROM users WHERE lower(username) = lower($1)"#,
        username,
    )
    .fetch_optional(pool)
    .await
}
