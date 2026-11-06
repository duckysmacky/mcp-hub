use anyhow::bail;
use clap::Subcommand;
use serde_json::json;
use sqlx::PgPool;
use zeroize::Zeroizing;

use super::format_time;
use crate::{
    audit,
    crypto::{INVITE_CODE_PREFIX, Token},
    db,
};

const MAX_INVITE_DAYS: u32 = 3650;

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum InviteCommand {
    /// Print a single-use sign-up code
    Create {
        /// How long the code stays valid
        #[arg(
            long,
            default_value_t = 7,
            value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_INVITE_DAYS))
        )]
        days: u32,
        /// A label to tell invites apart in `invite list`
        #[arg(long)]
        note: Option<String>,
    },
    /// List invites
    List,
    /// Revoke an invite; only an unused one can be revoked
    Revoke { id: i64 },
}

pub(super) async fn run(command: InviteCommand, pool: &PgPool) -> anyhow::Result<()> {
    match command {
        InviteCommand::Create { days, note } => {
            let code = create_invite(pool, days, note.as_deref()).await?;
            println!("invite code (valid {days} days): {}", code.as_str());
            println!("it is not shown again");
        }
        InviteCommand::List => {
            for invite in db::invites::list(pool).await? {
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
        InviteCommand::Revoke { id } => {
            if !db::invites::revoke(pool, id).await? {
                bail!("no unused invite with id {id}");
            }
            audit::record(
                pool,
                None,
                None,
                "invite_revoked",
                Some(json!({ "invite_id": id })),
            )
            .await;
            println!("invite {id} is revoked");
        }
    }
    Ok(())
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
