//! Operator commands. There are no admin endpoints: whoever can run the binary next to the
//! hub's config is the operator (D18).

use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

use anyhow::{Context, bail};
use serde_json::json;
use sqlx::PgPool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

use crate::{
    audit, auth,
    catalog::{
        Catalog,
        install::{effective_lock, generate_lock, reinstall_server},
        lock::CatalogLock,
    },
    config::Config,
    crypto::{self, Dek, INVITE_CODE_PREFIX, KdfParams, Token, WrapKind},
    db,
    supervisor::STOP_CHANNEL,
};

pub const USAGE: &str = "usage: mcp-hub [--settings <path>] [command]

With no command, runs the server.

commands:
  user list
  user disable <username>         log a user out everywhere and revoke their grants
  user enable <username>
  user reset-password <username> [--yes]
                                  wipes that user's stored values and grants
  invite create [--days <n>] [--note <text>]
                                  print a single-use sign-up code (default: valid 7 days)
  invite list
  invite revoke <id>              only an unused invite can be revoked
  catalog lock [--output <path>]  resolve and record every enabled server's dependencies
  catalog reinstall [<server-id>] reinstall from the lockfile, stopping running processes";

const DEFAULT_INVITE_DAYS: u32 = 7;
const MAX_INVITE_DAYS: u32 = 3650;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    UserList,
    UserDisable(String),
    UserEnable(String),
    UserResetPassword {
        username: String,
        confirmed: bool,
    },
    InviteCreate {
        valid_days: u32,
        note: Option<String>,
    },
    InviteList,
    InviteRevoke(i64),
    CatalogLock {
        output: Option<PathBuf>,
    },
    CatalogReinstall {
        server_id: Option<String>,
    },
}

/// Parses arguments into the settings path and the command to run.
pub fn parse(args: &[String]) -> anyhow::Result<(Option<PathBuf>, Command)> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (settings, rest) = match args.as_slice() {
        ["--settings", path, rest @ ..] => (Some(PathBuf::from(path)), rest),
        ["--settings"] => bail!("--settings needs a path"),
        rest => (None, rest),
    };
    let command = match rest {
        [] => Command::Serve,
        ["user", "list"] => Command::UserList,
        ["user", "disable", username] => Command::UserDisable((*username).to_owned()),
        ["user", "enable", username] => Command::UserEnable((*username).to_owned()),
        ["user", "reset-password", username] => Command::UserResetPassword {
            username: (*username).to_owned(),
            confirmed: false,
        },
        ["user", "reset-password", username, "--yes"] => Command::UserResetPassword {
            username: (*username).to_owned(),
            confirmed: true,
        },
        ["invite", "create", flags @ ..] => parse_invite_create(flags)?,
        ["invite", "list"] => Command::InviteList,
        ["invite", "revoke", id] => {
            Command::InviteRevoke(id.parse().context("id must be a number")?)
        }
        ["catalog", "lock"] => Command::CatalogLock { output: None },
        ["catalog", "lock", "--output", path] => Command::CatalogLock {
            output: Some(PathBuf::from(path)),
        },
        ["catalog", "reinstall"] => Command::CatalogReinstall { server_id: None },
        ["catalog", "reinstall", server_id] => Command::CatalogReinstall {
            server_id: Some((*server_id).to_owned()),
        },
        _ => bail!("{USAGE}"),
    };
    Ok((settings, command))
}

fn parse_invite_create(mut flags: &[&str]) -> anyhow::Result<Command> {
    let mut valid_days = DEFAULT_INVITE_DAYS;
    let mut note = None;
    loop {
        match flags {
            [] => return Ok(Command::InviteCreate { valid_days, note }),
            ["--days", days, rest @ ..] => {
                valid_days = days
                    .parse()
                    .ok()
                    .filter(|days| (1..=MAX_INVITE_DAYS).contains(days))
                    .with_context(|| format!("--days must be 1-{MAX_INVITE_DAYS}"))?;
                flags = rest;
            }
            ["--note", text, rest @ ..] => {
                note = Some((*text).to_owned());
                flags = rest;
            }
            _ => bail!("{USAGE}"),
        }
    }
}

/// Runs an operator command. Results go to stdout, logs to stderr.
pub async fn run(command: Command, config: &Config) -> anyhow::Result<()> {
    // Locking is done ahead of deploying, so it must not need the database.
    if let Command::CatalogLock { output } = command {
        return lock_catalog(config, output).await;
    }
    let pool = db::connect(config.database.clone()).await?;
    db::migrate(&pool).await?;
    match command {
        Command::Serve | Command::CatalogLock { .. } => {
            bail!("not an operator command that needs the database")
        }
        Command::InviteCreate { valid_days, note } => {
            let code = create_invite(&pool, valid_days, note.as_deref()).await?;
            println!("invite code (valid {valid_days} days): {}", code.as_str());
            println!("it is not shown again");
        }
        Command::InviteList => {
            for invite in db::invites::list(&pool).await? {
                let status = match invite.used_at {
                    Some(at) => format!("used {}", format_time(at)),
                    None => format!("expires {}", format_time(invite.expires_at)),
                };
                println!(
                    "{}\tcreated {}\t{status}\t{}",
                    invite.id,
                    format_time(invite.created_at),
                    invite.note.unwrap_or_default()
                );
            }
        }
        Command::InviteRevoke(id) => {
            if !db::invites::revoke(&pool, id).await? {
                bail!("no unused invite with id {id}");
            }
            audit::record(
                &pool,
                None,
                None,
                "invite_revoked",
                Some(json!({ "invite_id": id })),
            )
            .await;
            println!("invite {id} is revoked");
        }
        Command::CatalogReinstall { server_id } => {
            reinstall_catalog(&pool, config, server_id.as_deref()).await?;
        }
        Command::UserList => {
            for user in db::users::list(&pool).await? {
                let status = match user.disabled_at {
                    Some(at) => format!("disabled {}", format_time(at)),
                    None => "active".to_owned(),
                };
                println!(
                    "{}\t{}\tcreated {}\t{status}",
                    user.id,
                    user.username,
                    format_time(user.created_at)
                );
            }
        }
        Command::UserDisable(username) => {
            disable_user(&pool, &username).await?;
            println!("{username} is disabled; their sessions and grants are gone");
        }
        Command::UserEnable(username) => {
            enable_user(&pool, &username).await?;
            println!("{username} can log in again; clients must reconnect");
        }
        Command::UserResetPassword {
            username,
            confirmed,
        } => {
            if !confirmed && !confirm_reset(&username)? {
                bail!("cancelled");
            }
            let params = config.settings.password_kdf.params();
            let password = reset_password(&pool, &username, params).await?;
            println!("new password for {username}: {}", password.as_str());
            println!("it is not shown again");
        }
    }
    Ok(())
}

async fn lock_catalog(config: &Config, output: Option<PathBuf>) -> anyhow::Result<()> {
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
    let path = output.unwrap_or_else(|| CatalogLock::path_for(&paths.catalog));
    let text = toml::to_string_pretty(&lock).context("serializing the lockfile")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}; commit it next to the catalog", path.display());
    Ok(())
}

async fn reinstall_catalog(
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

fn format_time(time: OffsetDateTime) -> String {
    time.format(&Rfc3339).unwrap_or_else(|_| time.to_string())
}

fn confirm_reset(username: &str) -> anyhow::Result<bool> {
    println!(
        "Resetting the password of {username} permanently deletes every value they stored \
         and revokes all their sessions and grants. Their old values can't be recovered."
    );
    print!("Type yes to continue: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim() == "yes")
}

async fn find_user_id(pool: &PgPool, username: &str) -> anyhow::Result<i64> {
    let user = db::users::find_by_username(pool, username)
        .await?
        .with_context(|| format!("no user named {username}"))?;
    Ok(user.id)
}

/// Creates an invite and returns its code. Only a hash of the code is stored.
pub async fn create_invite(
    pool: &PgPool,
    valid_days: u32,
    note: Option<&str>,
) -> anyhow::Result<Zeroizing<String>> {
    let code = Token::generate();
    let id = db::invites::create(pool, &code.hash(), note, valid_days).await?;
    audit::record(
        pool,
        None,
        None,
        "invite_created",
        Some(json!({ "invite_id": id })),
    )
    .await;
    Ok(code.encode(INVITE_CODE_PREFIX))
}

/// Asks the running hub, if there is one, to kill matching processes.
pub async fn notify_stop(pool: &PgPool, payload: &str) -> anyhow::Result<()> {
    sqlx::query!("SELECT pg_notify($1, $2)", STOP_CHANNEL, payload)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn disable_user(pool: &PgPool, username: &str) -> anyhow::Result<()> {
    let user_id = find_user_id(pool, username).await?;
    db::users::disable(pool, user_id).await?;
    audit::record(pool, Some(user_id), None, "user_disabled", None).await;
    notify_stop(pool, &format!("user:{user_id}")).await
}

pub async fn enable_user(pool: &PgPool, username: &str) -> anyhow::Result<()> {
    let user_id = find_user_id(pool, username).await?;
    db::users::enable(pool, user_id).await?;
    audit::record(pool, Some(user_id), None, "user_enabled", None).await;
    Ok(())
}

/// Replaces a user's password and DEK, and returns the generated password.
pub async fn reset_password(
    pool: &PgPool,
    username: &str,
    params: KdfParams,
) -> anyhow::Result<Zeroizing<String>> {
    let user_id = find_user_id(pool, username).await?;
    let password = Token::generate().encode("");
    let kek_salt = crypto::generate_salt();
    let (password_hash, kek) = auth::hash_new_password(password.clone(), kek_salt.clone(), params)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let wrapped_dek =
        crypto::wrap_dek(&kek, &Dek::generate(), WrapKind::Password, user_id, user_id)?;
    db::users::reset_password(
        pool,
        user_id,
        db::users::NewPassword {
            password_hash: &password_hash,
            kek_salt: &kek_salt,
            kek_params: params,
            wrapped_dek: &wrapped_dek,
        },
    )
    .await?;
    audit::record(pool, Some(user_id), None, "password_reset", None).await;
    notify_stop(pool, &format!("user:{user_id}")).await?;
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> anyhow::Result<(Option<PathBuf>, Command)> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        parse(&args)
    }

    #[test]
    fn arguments_select_a_command() {
        assert_eq!(parsed(&[]).unwrap(), (None, Command::Serve));
        assert_eq!(
            parsed(&["--settings", "a.toml"]).unwrap(),
            (Some(PathBuf::from("a.toml")), Command::Serve)
        );
        assert_eq!(
            parsed(&["--settings", "a.toml", "user", "disable", "bob"]).unwrap(),
            (
                Some(PathBuf::from("a.toml")),
                Command::UserDisable("bob".to_owned())
            )
        );
        assert_eq!(
            parsed(&["user", "reset-password", "bob", "--yes"])
                .unwrap()
                .1,
            Command::UserResetPassword {
                username: "bob".to_owned(),
                confirmed: true
            }
        );
        assert_eq!(
            parsed(&["invite", "create", "--note", "for bob", "--days", "3"])
                .unwrap()
                .1,
            Command::InviteCreate {
                valid_days: 3,
                note: Some("for bob".to_owned())
            }
        );
        assert_eq!(
            parsed(&["invite", "create"]).unwrap().1,
            Command::InviteCreate {
                valid_days: DEFAULT_INVITE_DAYS,
                note: None
            }
        );
        for invalid in [
            &["invite", "create", "--days", "0"][..],
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
