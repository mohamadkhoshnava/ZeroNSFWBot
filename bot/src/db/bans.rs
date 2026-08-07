use anyhow::Result;
use sqlx::PgPool;

use super::models::StoredReason;

pub async fn record(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    score: f32,
    reasons: &[StoredReason],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO bans (chat_id, user_id, score, reasons)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (chat_id, user_id) DO UPDATE
            SET score = EXCLUDED.score,
                reasons = EXCLUDED.reasons,
                banned_at = now(),
                -- Re-banning must clear a previous pardon, or the shared
                -- blocklist would keep ignoring this group's judgement.
                unbanned_at = NULL,
                unbanned_by = NULL
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(score)
    .bind(serde_json::to_value(reasons)?)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_unban(pool: &PgPool, chat_id: i64, user_id: i64, by: i64) -> Result<()> {
    sqlx::query(
        "UPDATE bans SET unbanned_at = now(), unbanned_by = $3 \
         WHERE chat_id = $1 AND user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(())
}

/// How many *other* groups currently have this user banned.
///
/// Excluding the calling group is what stops the shared blocklist from
/// reinforcing its own mistakes: a single false positive in one group can never
/// grow into a self-justifying global ban.
pub async fn other_group_bans(pool: &PgPool, user_id: i64, exclude_chat: i64) -> Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM bans \
         WHERE user_id = $1 AND chat_id <> $2 AND unbanned_at IS NULL",
    )
    .bind(user_id)
    .bind(exclude_chat)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}
