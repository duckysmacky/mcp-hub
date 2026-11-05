pub mod instances;
pub mod invites;
pub mod oauth;
pub mod users;
pub mod web_sessions;

use std::time::{Duration, Instant};

use anyhow::Context;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

const CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(60);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Connects to Postgres, retrying so that startup order doesn't matter.
pub async fn connect(options: PgConnectOptions) -> anyhow::Result<PgPool> {
    let started = Instant::now();
    loop {
        match PgPoolOptions::new().connect_with(options.clone()).await {
            Ok(pool) => return Ok(pool),
            Err(error) if started.elapsed() < CONNECT_RETRY_WINDOW => {
                tracing::warn!(%error, "database not reachable yet, retrying");
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error).context("connecting to the database"),
        }
    }
}

/// Applies any migrations that haven't run yet.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!()
        .run(pool)
        .await
        .context("running migrations")
}
