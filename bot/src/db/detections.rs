use anyhow::Result;
use sqlx::PgPool;

use super::models::{DetectionRow, NewDetection};

/// Persist a detection and return its id, which becomes the callback payload
/// for the "false positive" and "details" buttons.
pub async fn insert(pool: &PgPool, detection: &NewDetection) -> Result<i64> {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO detections
            (chat_id, user_id, message_id, score, verdict, banned, deleted, muted, dry_run, reasons)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id
        "#,
    )
    .bind(detection.chat_id)
    .bind(detection.user_id)
    .bind(detection.message_id)
    .bind(detection.score)
    .bind(detection.verdict.as_str())
    .bind(detection.banned)
    .bind(detection.deleted)
    .bind(detection.muted)
    .bind(detection.dry_run)
    .bind(serde_json::to_value(&detection.reasons)?)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn get(pool: &PgPool, id: i64) -> Result<Option<DetectionRow>> {
    Ok(sqlx::query_as(
        "SELECT id, chat_id, user_id, score, banned, reasons FROM detections WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Flag a detection as a mistake. The stored value is what makes it possible to
/// measure the filters' precision later instead of guessing at it.
pub async fn mark_incorrect(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("UPDATE detections SET is_correct = FALSE WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
