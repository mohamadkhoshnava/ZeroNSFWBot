use anyhow::Result;
use sqlx::PgPool;

/// File an appeal. Returns `false` if one is already open, which the caller
/// turns into "you already appealed" rather than a silent duplicate.
pub async fn open(pool: &PgPool, chat_id: i64, user_id: i64) -> Result<bool> {
    let result = sqlx::query(
        "INSERT INTO appeals (chat_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(chat_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn resolve(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    approved: bool,
    admin_id: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE appeals SET status = $3, resolved_at = now(), resolved_by = $4 \
         WHERE chat_id = $1 AND user_id = $2 AND status = 'open'",
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(if approved { "approved" } else { "rejected" })
    .bind(admin_id)
    .execute(pool)
    .await?;
    Ok(())
}
