use std::{collections::HashMap, sync::Arc};

use axum::routing::MethodRouter;
use sqlx::PgPool;
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::{
    catalog::{Catalog, install::ResolvedServer},
    config::Config,
    rate_limit::{RateLimiter, limit_by_ip},
    supervisor::Supervisor,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub catalog: Arc<Catalog>,
    /// Catalog server id to what its processes run.
    pub bin_scripts: Arc<HashMap<String, ResolvedServer>>,
    pub supervisor: Arc<Supervisor>,
    pub rate_limiter: Arc<RateLimiter>,
    password_hashes: Arc<Semaphore>,
}

impl AppState {
    pub fn new(
        config: Arc<Config>,
        pool: PgPool,
        catalog: Catalog,
        bin_scripts: HashMap<String, ResolvedServer>,
        supervisor: Arc<Supervisor>,
    ) -> Self {
        let max_hashes = config.settings.limits.max_concurrent_password_hashes;
        Self {
            config,
            pool,
            catalog: Arc::new(catalog),
            bin_scripts: Arc::new(bin_scripts),
            supervisor,
            rate_limiter: Arc::default(),
            password_hashes: Arc::new(Semaphore::new(max_hashes)),
        }
    }

    /// Waits for a slot to hash a password in. Hold it for as long as the hashing runs.
    pub async fn password_hash_slot(&self) -> SemaphorePermit<'_> {
        self.password_hashes
            .acquire()
            .await
            .expect("this semaphore is never closed")
    }

    /// Limits a route to `limit` requests per client address per minute.
    pub fn limited(
        &self,
        bucket: &'static str,
        limit: u32,
        route: MethodRouter<AppState>,
    ) -> MethodRouter<AppState> {
        route.layer(axum::middleware::from_fn_with_state(
            (self.clone(), bucket, limit),
            limit_by_ip,
        ))
    }
}
