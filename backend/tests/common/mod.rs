use std::sync::Arc;

use mcp_hub::{
    app_state::AppState,
    config::{Config, Settings},
};
use sqlx::{PgPool, postgres::PgConnectOptions};

pub fn state(pool: PgPool) -> AppState {
    AppState {
        config: Arc::new(Config {
            public_url: "http://localhost:8080".parse().unwrap(),
            allow_signup: true,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database: PgConnectOptions::new(),
            settings: Settings::default(),
        }),
        pool,
    }
}
