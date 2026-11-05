use sqlx::PgPool;
use time::OffsetDateTime;

pub struct Invite {
    pub id: i64,
    pub note: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub used_at: Option<OffsetDateTime>,
}

pub async fn create(
    pool: &PgPool,
    code_hash: &[u8],
    note: Option<&str>,
    valid_days: u32,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO invites (code_hash, note, expires_at)
         VALUES ($1, $2, now() + make_interval(days => $3))
         RETURNING id",
        code_hash,
        note,
        valid_days as i32,
    )
    .fetch_one(pool)
    .await
}

pub async fn list(pool: &PgPool) -> Result<Vec<Invite>, sqlx::Error> {
    sqlx::query_as!(
        Invite,
        "SELECT id, note, created_at, expires_at, used_at FROM invites ORDER BY id"
    )
    .fetch_all(pool)
    .await
}

/// Removes an invite nobody has used yet. Returns false if there is no such invite.
pub async fn revoke(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM invites WHERE id = $1 AND used_at IS NULL", id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Spares a bad code the password hashing. Signing up still claims the invite atomically.
pub async fn is_usable(pool: &PgPool, code_hash: &[u8]) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM invites
               WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
           ) AS "exists!""#,
        code_hash,
    )
    .fetch_one(pool)
    .await
}
