//! The audit trail. It records what happened, never with what: no env values, tool
//! arguments, tokens or passwords in `detail`.

use serde_json::Value;
use sqlx::{PgPool, types::Json};

const MAX_DETAIL_TEXT_CHARS: usize = 256;

/// Records an event. A failure is logged and never fails the request being audited.
pub async fn record(
    pool: &PgPool,
    user_id: Option<i64>,
    instance_id: Option<i64>,
    event: &str,
    detail: Option<Value>,
) {
    let result = sqlx::query!(
        "INSERT INTO audit_events (user_id, instance_id, event, detail) VALUES ($1, $2, $3, $4)",
        user_id,
        instance_id,
        event,
        detail.map(Json) as _,
    )
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::error!(%error, event, "could not record an audit event");
    }
}

/// Bounds text that came from a client, such as a tool name.
pub fn bounded(text: &str) -> String {
    text.chars().take(MAX_DETAIL_TEXT_CHARS).collect()
}
