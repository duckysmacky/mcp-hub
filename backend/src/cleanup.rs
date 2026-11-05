//! Deletes what's no longer needed, so that the tables stay small and old DEK wraps go away.

use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Removes expired rows. Some outlive their expiry on purpose:
/// - a code's row is what detects its replay, so it's kept for a day after expiring,
/// - a rotated refresh token is what detects its reuse, so it's kept until it expires,
/// - a revoked grant is kept for a month as a record of what happened.
pub async fn run_once(pool: &PgPool, audit_retention_days: u32) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM oauth_pending_authorizations
         WHERE consumed_at IS NOT NULL OR expires_at < now()"
    )
    .execute(pool)
    .await?;
    sqlx::query!("DELETE FROM oauth_codes WHERE expires_at < now() - interval '1 day'")
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM oauth_tokens WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM oauth_grants WHERE revoked_at < now() - interval '30 days'")
        .execute(pool)
        .await?;
    // Registration is open, so clients nobody ever authorized must not pile up forever.
    sqlx::query!(
        "DELETE FROM oauth_clients AS client
         WHERE client.created_at < now() - interval '1 day'
           AND NOT EXISTS (SELECT 1 FROM oauth_grants WHERE client_id = client.client_id)
           AND NOT EXISTS (
               SELECT 1 FROM oauth_pending_authorizations WHERE client_id = client.client_id
           )"
    )
    .execute(pool)
    .await?;
    sqlx::query!("DELETE FROM web_sessions WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM invites WHERE used_at IS NULL AND expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query!(
        "DELETE FROM audit_events WHERE at < now() - make_interval(days => $1)",
        audit_retention_days as i32,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Runs the cleanup every hour until `shutdown` fires.
pub fn spawn(pool: PgPool, audit_retention_days: u32, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {
                    if let Err(error) = run_once(&pool, audit_retention_days).await {
                        tracing::error!(%error, "cleanup failed");
                    }
                }
            }
        }
    });
}
