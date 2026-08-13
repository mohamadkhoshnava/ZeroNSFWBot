use anyhow::Result;
use sqlx::PgPool;

use super::models::ScanCacheRow;

/// Identifies the scoring pipeline whose output is cached.
///
/// Bump this whenever a change alters what a cached row *means* — a new model,
/// a new stage, a different column semantics. The photo fingerprint only
/// detects a changed image; without this, a changed scorer would keep serving
/// stale verdicts to every account already in the cache.
///
/// `v3` is where `nsfw_score` stopped being the final verdict and became the
/// screening score, with `verifier_labels` alongside it.
pub const PIPELINE_VERSION: &str = "v3-per-group-categories";

/// Expands to the column list `ScanCacheRow` expects. A macro so the query
/// stays a `&'static str` literal, which is what lets sqlx accept it without an
/// injection-safety assertion.
macro_rules! cache_columns {
    () => {
        "photo_fingerprint, nsfw_score, has_photo, bio, ocr_text, verifier_labels"
    };
}

/// Look up a previous scan, but only accept it if the profile photos are still
/// the same ones *and* the same pipeline produced the row.
///
/// `fingerprint` is built from Telegram's `file_unique_id`s, which change the
/// moment a user swaps their avatar. That makes staleness impossible to get
/// wrong: a mismatch is a miss, so a spammer cannot hide behind a cached score
/// by changing their picture.
pub async fn get(pool: &PgPool, user_id: i64, fingerprint: &str) -> Result<Option<ScanCacheRow>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        cache_columns!(),
        " FROM scan_cache WHERE user_id = $1 AND photo_fingerprint = $2 AND pipeline = $3"
    ))
    .bind(user_id)
    .bind(fingerprint)
    .bind(PIPELINE_VERSION)
    .fetch_optional(pool)
    .await?)
}

/// Store the evidence from a scan.
///
/// `verifier_labels` is `None` when the screening score fell below the group's
/// threshold and the second stage never ran. A later group with a lower
/// threshold verifies the same account and fills it in.
#[allow(clippy::too_many_arguments)]
pub async fn put(
    pool: &PgPool,
    user_id: i64,
    fingerprint: &str,
    nsfw_score: f32,
    has_photo: bool,
    bio: Option<&str>,
    ocr_text: Option<&str>,
    verifier_labels: Option<&[(String, f32)]>,
) -> Result<()> {
    let labels = verifier_labels
        .map(|pairs| {
            serde_json::to_value(
                pairs
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashMap<_, _>>(),
            )
        })
        .transpose()?;

    sqlx::query(
        r#"
        INSERT INTO scan_cache
            (user_id, photo_fingerprint, nsfw_score, has_photo, bio, ocr_text,
             pipeline, verifier_labels)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (user_id) DO UPDATE
            SET photo_fingerprint = EXCLUDED.photo_fingerprint,
                nsfw_score        = EXCLUDED.nsfw_score,
                has_photo         = EXCLUDED.has_photo,
                bio               = EXCLUDED.bio,
                ocr_text          = EXCLUDED.ocr_text,
                pipeline          = EXCLUDED.pipeline,
                -- Never overwrite a breakdown with NULL: a group whose
                -- threshold skipped verification would otherwise erase what a
                -- stricter group already established about the same account.
                verifier_labels   = COALESCE(EXCLUDED.verifier_labels,
                                             scan_cache.verifier_labels),
                scanned_at        = now()
        "#,
    )
    .bind(user_id)
    .bind(fingerprint)
    .bind(nsfw_score)
    .bind(has_photo)
    .bind(bio)
    .bind(ocr_text)
    .bind(PIPELINE_VERSION)
    .bind(labels)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop entries older than `days`, so a profile is eventually re-checked even
/// if its photo never changes and the models have improved since.
///
/// Also sweeps rows left behind by a previous pipeline, which can never be read
/// again and would otherwise sit there until their age caught up with them.
pub async fn prune(pool: &PgPool, days: i64) -> Result<u64> {
    // make_interval takes the count as a bind parameter, so the query stays a
    // static string with no interpolation.
    let result = sqlx::query(
        "DELETE FROM scan_cache \
         WHERE scanned_at < now() - make_interval(days => $1) OR pipeline <> $2",
    )
    .bind(days.clamp(1, 3650) as i32)
    .bind(PIPELINE_VERSION)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
