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
