use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use mcp_hub::{
    app_state::AppState,
    build_router,
    config::{Config, Settings},
    db,
};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

const DEFAULT_SETTINGS_PATH: &str = "/etc/mcp-hub/settings.toml";
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings_arg = parse_settings_arg()?;
    // In production the variables come from the container's environment.
    let _ = dotenvy::dotenv();
    init_tracing();

    let settings = match &settings_arg {
        Some(path) => Settings::load(path, true)?,
        None => Settings::load(DEFAULT_SETTINGS_PATH.as_ref(), false)?,
    };
    let config = Arc::new(Config::from_env(settings)?);
    let pool = db::connect(config.database.clone()).await?;

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!(address = %config.bind_addr, public_url = config.public_url.as_str(), "listening");

    let state = AppState {
        config,
        pool: pool.clone(),
    };
    let (drain_started, drain_signal) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, build_router(state)).with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _ = drain_started.send(());
    });

    tokio::select! {
        result = server.into_future() => result.context("serving HTTP")?,
        () = async {
            let _ = drain_signal.await;
            tokio::time::sleep(SHUTDOWN_DRAIN_TIMEOUT).await;
        } => tracing::warn!("connections still open after the drain timeout"),
    }

    pool.close().await;
    Ok(())
}

fn parse_settings_arg() -> anyhow::Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    let settings_path = match args.next().as_deref() {
        None => None,
        Some("--settings") => Some(PathBuf::from(
            args.next().context("--settings needs a path")?,
        )),
        Some(other) => bail!("unknown argument {other}"),
    };
    if args.next().is_some() {
        bail!("too many arguments");
    }
    Ok(settings_path)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter);
    if cfg!(debug_assertions) {
        subscriber.pretty().init();
    } else {
        subscriber.json().init();
    }
}

async fn shutdown_signal() {
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        tracing::error!("could not install the SIGTERM handler");
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
