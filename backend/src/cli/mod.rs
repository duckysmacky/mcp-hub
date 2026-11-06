//! Command line: the server (`run`) and the operator commands. There are no admin endpoints:
//! whoever can run the binary next to the hub's config is the operator (D18).

mod catalog;
mod invite;
mod user;

use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};
use sqlx::PgPool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{config::Config, db, supervisor::STOP_CHANNEL};

pub use catalog::CatalogCommand;
pub use invite::{InviteCommand, create_invite};
pub use user::{UserCommand, disable_user, enable_user, reset_password};

#[derive(Debug, Parser)]
#[command(version, about, arg_required_else_help = true)]
pub struct Cli {
    /// Settings file (default: /etc/mcp-hub/settings.toml, optional)
    #[arg(long, global = true, value_name = "PATH")]
    pub settings: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Run the server
    Run,
    /// Manage users
    #[command(subcommand)]
    User(UserCommand),
    /// Manage sign-up invites
    #[command(subcommand)]
    Invite(InviteCommand),
    /// Manage the catalog's installed servers and lockfile
    #[command(subcommand)]
    Catalog(CatalogCommand),
}

/// Runs an operator command. Results go to stdout, logs to stderr.
pub async fn run(command: Command, config: &Config) -> anyhow::Result<()> {
    // Locking is done ahead of deploying, so it must not need the database.
    if let Command::Catalog(CatalogCommand::Lock { output }) = command {
        return catalog::lock(config, output).await;
    }
    let pool = db::connect(config.database.clone()).await?;
    db::migrate(&pool).await?;
    match command {
        Command::Run | Command::Catalog(CatalogCommand::Lock { .. }) => {
            bail!("not an operator command that needs the database")
        }
        Command::User(command) => user::run(command, &pool, config).await,
        Command::Invite(command) => invite::run(command, &pool).await,
        Command::Catalog(CatalogCommand::Reinstall { server_id }) => {
            catalog::reinstall(&pool, config, server_id.as_deref()).await
        }
    }
}

/// Asks the running hub, if there is one, to kill matching processes.
pub async fn notify_stop(pool: &PgPool, payload: &str) -> anyhow::Result<()> {
    sqlx::query!("SELECT pg_notify($1, $2)", STOP_CHANNEL, payload)
        .execute(pool)
        .await?;
    Ok(())
}

fn format_time(time: OffsetDateTime) -> String {
    time.format(&Rfc3339).unwrap_or_else(|_| time.to_string())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parsed(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("mcp-hub").chain(args.iter().copied()))
    }

    #[test]
    fn definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn arguments_select_a_command() {
        let cli = parsed(&["run"]).unwrap();
        assert_eq!((cli.settings, cli.command), (None, Command::Run));
        // Global: accepted on either side of the subcommand.
        for args in [
            &["--settings", "a.toml", "user", "disable", "bob"][..],
            &["user", "disable", "bob", "--settings", "a.toml"],
        ] {
            let cli = parsed(args).unwrap();
            assert_eq!(cli.settings, Some(PathBuf::from("a.toml")));
            assert_eq!(
                cli.command,
                Command::User(UserCommand::Disable {
                    username: "bob".to_owned()
                })
            );
        }
        assert_eq!(
            parsed(&["user", "reset-password", "bob", "--yes"])
                .unwrap()
                .command,
            Command::User(UserCommand::ResetPassword {
                username: "bob".to_owned(),
                yes: true
            })
        );
        assert_eq!(
            parsed(&["invite", "create", "--note", "for bob", "--days", "3"])
                .unwrap()
                .command,
            Command::Invite(InviteCommand::Create {
                days: 3,
                note: Some("for bob".to_owned())
            })
        );
        assert_eq!(
            parsed(&["invite", "create"]).unwrap().command,
            Command::Invite(InviteCommand::Create {
                days: 7,
                note: None
            })
        );
        assert_eq!(
            parsed(&["catalog", "lock", "--output", "-"])
                .unwrap()
                .command,
            Command::Catalog(CatalogCommand::Lock {
                output: Some(PathBuf::from("-"))
            })
        );
        assert_eq!(
            parsed(&["catalog", "reinstall"]).unwrap().command,
            Command::Catalog(CatalogCommand::Reinstall { server_id: None })
        );
    }

    #[test]
    fn invalid_arguments_are_refused() {
        for invalid in [
            &[][..],
            &["--settings", "a.toml"],
            &["invite", "create", "--days", "0"],
            &["invite", "create", "--days", "3651"],
            &["invite", "create", "--days", "x"],
            &["invite", "revoke", "x"],
            &["--settings"],
            &["nonsense"],
            &["user"],
            &["user", "disable"],
            &["user", "reset-password", "bob", "--force"],
        ] {
            assert!(parsed(invalid).is_err(), "{invalid:?}");
        }
    }
}
