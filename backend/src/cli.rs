//! Operator commands. There are no admin endpoints: whoever can run the binary next to the
//! hub's config is the operator (D18).

use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

use anyhow::{Context, bail};
use sqlx::PgPool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

use crate::{
    audit, auth,
    config::Config,
    crypto::{self, Dek, KdfParams, Token, WrapKind},
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
                                  wipes that user's stored values and grants";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    UserList,
    UserDisable(String),
    UserEnable(String),
    UserResetPassword { username: String, confirmed: bool },
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
        _ => bail!("{USAGE}"),
    };
    Ok((settings, command))
}

/// Runs an operator command. Results go to stdout, logs to stderr.
pub async fn run(command: Command, config: &Config) -> anyhow::Result<()> {
    let pool = db::connect(config.database.clone()).await?;
    db::migrate(&pool).await?;
    match command {
        Command::Serve => bail!("serving is not an operator command"),
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
        for invalid in [
            &["--settings"][..],
            &["nonsense"],
            &["user"],
            &["user", "disable"],
            &["user", "reset-password", "bob", "--force"],
        ] {
            assert!(parsed(invalid).is_err(), "{invalid:?}");
        }
    }
}
