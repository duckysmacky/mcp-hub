use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use axum::routing::MethodRouter;
use sqlx::PgPool;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

use crate::{
    catalog::{
        Catalog, ServerTemplate,
        install::{self, InstallError, ResolvedServer},
    },
    config::Config,
    db::instances::Instance,
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
    // ponytail: one custom install at a time across all users, and a slow one holds up the
    // rest. Fine for the small hubs this is meant for; use per-instance locks if it isn't.
    custom_installs: Arc<Mutex<()>>,
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
            custom_installs: Arc::default(),
        }
    }

    /// What an instance runs. `None` once its catalog server is gone, or for a custom instance
    /// while the operator has custom servers turned off.
    pub fn template_for(&self, instance: &Instance) -> Option<Cow<'_, ServerTemplate>> {
        match &instance.custom {
            Some(custom) => self
                .config
                .settings
                .catalog
                .allow_custom_servers
                .then(|| Cow::Owned(custom.0.to_template(instance.id))),
            None => self.catalog.get(&instance.server_id).map(Cow::Borrowed),
        }
    }

    pub async fn install_custom(
        &self,
        server: &ServerTemplate,
    ) -> Result<ResolvedServer, InstallError> {
        let _guard = self.custom_installs.lock().await;
        self.install_custom_locked(server).await
    }

    /// What a custom server runs, installing it if needed: the packages volume may be new.
    pub async fn resolved_custom(
        &self,
        server: &ServerTemplate,
    ) -> Result<ResolvedServer, InstallError> {
        let _guard = self.custom_installs.lock().await;
        match install::resolve_custom(server, &self.config.settings) {
            Ok(resolved) => Ok(resolved),
            Err(_) => self.install_custom_locked(server).await,
        }
    }

    /// The package tools are killed when the timeout drops them.
    async fn install_custom_locked(
        &self,
        server: &ServerTemplate,
    ) -> Result<ResolvedServer, InstallError> {
        let settings = &self.config.settings;
        let limit = Duration::from_secs(settings.catalog.custom_install_timeout_secs);
        tokio::time::timeout(limit, install::install_custom(server, settings))
            .await
            .unwrap_or_else(|_| {
                Err(InstallError::Failed {
                    server: server.id.clone(),
                    reason: "installing took too long".to_owned(),
                })
            })
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
