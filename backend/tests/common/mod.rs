#![allow(dead_code)] // Each test crate uses a different part of this module.

use std::{collections::HashMap, sync::Arc};
use std::{path::PathBuf, process::Command, sync::OnceLock};

use mcp_hub::{
    app_state::AppState,
    catalog::Catalog,
    config::{Config, Settings},
    supervisor::Supervisor,
};
use sqlx::{PgPool, postgres::PgConnectOptions};

pub fn config(settings: Settings) -> Arc<Config> {
    Arc::new(Config {
        public_url: "http://localhost:8080".parse().unwrap(),
        allow_signup: true,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        database: PgConnectOptions::new(),
        settings,
    })
}

pub fn state(pool: PgPool) -> AppState {
    let config = config(Settings::default());
    AppState {
        supervisor: Supervisor::new(config.clone()),
        config,
        pool,
        catalog: Arc::new(Catalog::parse("").unwrap()),
        bin_scripts: Arc::new(HashMap::new()),
    }
}

/// Path of the fake MCP server. A plain `cargo test` builds it; it's built here only
/// for a filtered run that skipped examples.
pub fn fake_mcp_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let find = || {
            std::env::current_exe()
                .unwrap()
                .ancestors()
                .map(|directory| directory.join("examples/fake_mcp"))
                .find(|candidate| candidate.exists())
        };
        find().unwrap_or_else(|| {
            let status = Command::new(env!("CARGO"))
                .args(["build", "--quiet", "--example", "fake_mcp"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .unwrap();
            assert!(status.success());
            find().expect("fake_mcp should exist under the target directory")
        })
    })
    .clone()
}
