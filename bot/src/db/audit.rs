use serde_json::Value;
use sqlx::PgPool;

/// Append to the audit trail.
///
/// Deliberately infallible from the caller's perspective: losing an audit row
/// must never abort a moderation action that already succeeded. Failures are
/// logged instead.
pub async fn log(
    pool: &PgPool,
    chat_id: Option<i64>,
    actor_id: Option<i64>,
    action: &str,
    payload: Value,
) {
    let result = sqlx::query(
        "INSERT INTO audit_log (chat_id, actor_id, action, payload) VALUES ($1, $2, $3, $4)",
    )
    .bind(chat_id)
    .bind(actor_id)
    .bind(action)
    .bind(payload)
    .execute(pool)
    .await;

    if let Err(err) = result {
        tracing::warn!(%err, action, "failed to write audit log entry");
    }
}
