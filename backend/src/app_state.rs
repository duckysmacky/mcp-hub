use std::{collections::HashMap, path::PathBuf, sync::Arc};

use sqlx::PgPool;

use crate::{catalog::Catalog, config::Config};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub catalog: Arc<Catalog>,
    /// Catalog server id to the installed script its processes run.
    pub bin_scripts: Arc<HashMap<String, PathBuf>>,
}
