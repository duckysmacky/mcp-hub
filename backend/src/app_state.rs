use std::{collections::HashMap, sync::Arc};

use sqlx::PgPool;

use crate::{
    catalog::{Catalog, install::ResolvedServer},
    config::Config,
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
}
