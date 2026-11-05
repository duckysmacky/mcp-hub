use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use mcp_hub::{
    app_state::AppState,
    build_router,
    catalog::{
        Catalog, ServerTemplate,
        install::{effective_lock, install_all},
    },
    cleanup,
    cli::{self, Command},
    config::{Config, Settings, SignupMode},
    db,
    supervisor::Supervisor,
};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, fmt::writer::BoxMakeWriter};

const DEFAULT_SETTINGS_PATH: &str = "/etc/mcp-hub/settings.toml";
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (settings_path, command) = cli::parse(&args)?;
    // In production the variables come from the container's environment.
    let _ = dotenvy::dotenv();
    init_tracing(command == Command::Serve);

    let settings = match &settings_path {
        Some(path) => Settings::load(path, true)?,
        None => Settings::load(DEFAULT_SETTINGS_PATH.as_ref(), false)?,
    };
    let config = Arc::new(Config::from_env(settings)?);
    match command {
        Command::Serve => serve(config).await,
        command => cli::run(command, &config).await,
    }
}

async fn serve(config: Arc<Config>) -> anyhow::Result<()> {
    if config.settings.server.signup == SignupMode::Enabled {
        // Same-UID stdio children can read each other's env (SECURITY.md T3).
        tracing::warn!("anyone who can reach this hub can sign up and run catalog servers");
    }
    let pool = db::connect(config.database.clone()).await?;
    db::migrate(&pool).await?;

    let catalog = Catalog::load(&config.settings.paths.catalog)?;
    if !config.settings.catalog.require_lock {
        tracing::warn!(
            "catalog.require_lock is off: servers without a lock entry get whatever their \
             dependencies resolve to at startup"
        );
    }
    let lock = effective_lock(&catalog, &config.settings).await?;
    let bin_scripts = install_all(
        &catalog,
        &lock,
        &config.settings.paths.packages,
        &config.settings.runtimes,
    )
    .await?;

    let (enabled, disabled): (Vec<_>, Vec<_>) =
        catalog.servers().partition(|server| server.enabled);
    let ids = |servers: Vec<&ServerTemplate>| -> Vec<String> {
        servers
            .into_iter()
            .map(|server| server.id.clone())
            .collect()
    };
    db::instances::sync_with_catalog(&pool, &ids(enabled), &ids(disabled)).await?;

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!(address = %config.bind_addr, public_url = config.public_url.as_str(), "listening");

    let supervisor = Supervisor::new(config.clone());
    supervisor.spawn_reaper();
    supervisor.spawn_stop_listener(pool.clone());
    let cleanup_shutdown = CancellationToken::new();
    cleanup::spawn(
        pool.clone(),
        config.settings.audit.retention_days,
        cleanup_shutdown.clone(),
    );
    let state = AppState::new(
        config,
        pool.clone(),
        catalog,
        bin_scripts,
        supervisor.clone(),
    );
    let (drain_started, drain_signal) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // Killing the children first also ends open SSE streams, so connections can drain.
        supervisor.shutdown().await;
        let _ = drain_started.send(());
    });

    tokio::select! {
        result = server.into_future() => result.context("serving HTTP")?,
        () = async {
            let _ = drain_signal.await;
            tokio::time::sleep(SHUTDOWN_DRAIN_TIMEOUT).await;
        } => tracing::warn!("connections still open after the drain timeout"),
    }

    cleanup_shutdown.cancel();
    pool.close().await;
    Ok(())
}

/// Operator commands log to stderr, so that their stdout is only their result.
fn init_tracing(to_stdout: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let writer = if to_stdout {
        BoxMakeWriter::new(std::io::stdout)
    } else {
        BoxMakeWriter::new(std::io::stderr)
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer);
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
