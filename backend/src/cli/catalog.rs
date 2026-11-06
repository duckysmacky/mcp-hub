use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Subcommand;
use sqlx::PgPool;

use super::notify_stop;
use crate::{
    catalog::{
        Catalog,
        install::{effective_lock, generate_lock, reinstall_server},
        lock::CatalogLock,
    },
    config::Config,
};

const STDOUT_OUTPUT: &str = "-";

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum CatalogCommand {
    /// Resolve and record every enabled server's dependencies
    Lock {
        /// Where to write the lock (default: next to the catalog); `-` prints it to stdout
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Reinstall from the lockfile, stopping running processes
    Reinstall {
        /// Only this server (default: every enabled one)
        server_id: Option<String>,
    },
}

pub(super) async fn lock(config: &Config, output: Option<PathBuf>) -> anyhow::Result<()> {
    let paths = &config.settings.paths;
    let catalog = Catalog::load(&paths.catalog)?;
    let work_dir = paths.scratch.join("catalog-lock");
    let lock = generate_lock(
        &catalog,
        &paths.packages,
        &work_dir,
        &config.settings.runtimes,
    )
    .await?;
    let text = toml::to_string_pretty(&lock).context("serializing the lockfile")?;
    // A container's user usually can't write to the operator's directory, but a shell can.
    if output.as_deref() == Some(Path::new(STDOUT_OUTPUT)) {
        print!("{text}");
        return Ok(());
    }
    let path = output.unwrap_or_else(|| CatalogLock::path_for(&paths.catalog));
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}; commit it next to the catalog", path.display());
    Ok(())
}

pub(super) async fn reinstall(
    pool: &PgPool,
    config: &Config,
    server_id: Option<&str>,
) -> anyhow::Result<()> {
    let paths = &config.settings.paths;
    let catalog = Catalog::load(&paths.catalog)?;
    let lock = effective_lock(&catalog, &config.settings).await?;
    if let Some(server_id) = server_id
        && !catalog.get(server_id).is_some_and(|server| server.enabled)
    {
        bail!("no enabled server named {server_id}");
    }
    let servers = catalog
        .servers()
        .filter(|server| server.enabled)
        .filter(|server| server_id.is_none_or(|wanted| wanted == server.id));
    for server in servers {
        // Stopped first: running processes read their files from the directory being replaced.
        notify_stop(pool, &format!("server:{}", server.id)).await?;
        reinstall_server(server, &lock, &paths.packages, &config.settings.runtimes).await?;
        println!("reinstalled {}", server.id);
    }
    Ok(())
}
