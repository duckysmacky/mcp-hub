use std::io::{BufRead, Write};

use anyhow::{Context, bail};
use clap::Subcommand;
use sqlx::PgPool;
use zeroize::Zeroizing;

use super::{format_time, notify_stop};
use crate::{
    audit, auth,
    config::Config,
    crypto::{self, Dek, KdfParams, Token, WrapKind},
    db,
};

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum UserCommand {
    /// List all users
    List,
    /// Log a user out everywhere and revoke their grants
    Disable { username: String },
    /// Let a disabled user log in again
    Enable { username: String },
    /// Wipe that user's stored values and grants, and print a new password
    ResetPassword {
        username: String,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

pub(super) async fn run(
    command: UserCommand,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<()> {
    match command {
        UserCommand::List => {
            for user in db::users::list(pool).await? {
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
        UserCommand::Disable { username } => {
            disable_user(pool, &username).await?;
            println!("{username} is disabled; their sessions and grants are gone");
        }
        UserCommand::Enable { username } => {
            enable_user(pool, &username).await?;
            println!("{username} can log in again; clients must reconnect");
        }
        UserCommand::ResetPassword { username, yes } => {
            if !yes && !confirm_reset(&username)? {
                bail!("cancelled");
            }
            let params = config.settings.password_kdf.params();
            let password = reset_password(pool, &username, params).await?;
            println!("new password for {username}: {}", password.as_str());
            println!("it is not shown again");
        }
    }
    Ok(())
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
