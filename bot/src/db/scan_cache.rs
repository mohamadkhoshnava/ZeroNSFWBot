use anyhow::Result;
use sqlx::PgPool;

use super::models::ScanCacheRow;

/// Look up a previous scan, but only accept it if the profile photos are still
/// the same ones.
///
/// `fingerprint` is built from Telegram's `file_unique_id`s, which change the
/// moment a user swaps their avatar. That makes staleness impossible to get
/// wrong: a mismatch is a miss, so a spammer cannot hide behind a cached score
/// by changing their picture.
pub async fn get(pool: &PgPool, user_id: i64, fingerprint: &str) -> Result<Option<ScanCacheRow>> {
    Ok(sqlx::query_as(
        "SELECT photo_fingerprint, nsfw_score, has_photo, bio, ocr_text \
         FROM scan_cache WHERE user_id = $1 AND photo_fingerprint = $2",
    )
    .bind(user_id)
    .bind(fingerprint)
    .fetch_optional(pool)
    .await?)
}

pub async fn put(
    pool: &PgPool,
    user_id: i64,
    fingerprint: &str,
    nsfw_score: f32,
    has_photo: bool,
    bio: Option<&str>,
    ocr_text: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO scan_cache (user_id, photo_fingerprint, nsfw_score, has_photo, bio, ocr_text)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_id) DO UPDATE
            SET photo_fingerprint = EXCLUDED.photo_fingerprint,
                nsfw_score        = EXCLUDED.nsfw_score,
                has_photo         = EXCLUDED.has_photo,
                bio               = EXCLUDED.bio,
                ocr_text          = EXCLUDED.ocr_text,
                scanned_at        = now()
        "#,
    )
    .bind(user_id)
    .bind(fingerprint)
    .bind(nsfw_score)
    .bind(has_photo)
    .bind(bio)
    .bind(ocr_text)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop entries older than `days`, so a profile is eventually re-checked even
/// if its photo never changes and the model has improved since.
pub async fn prune(pool: &PgPool, days: i64) -> Result<u64> {
    // make_interval takes the count as a bind parameter, so the query stays a
    // static string with no interpolation.
    let result =
        sqlx::query("DELETE FROM scan_cache WHERE scanned_at < now() - make_interval(days => $1)")
            .bind(days.clamp(1, 3650) as i32)
            .execute(pool)
            .await?;
    Ok(result.rows_affected())
}
