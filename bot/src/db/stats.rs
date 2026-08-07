use anyhow::Result;
use sqlx::PgPool;

use super::models::{GlobalStats, PeriodStats, StatsBundle};

/// Per-group counts for `/nsfw → Statistics`.
///
/// One query with conditional aggregates rather than four round trips: the
/// panel is opened interactively and the difference is visible.
pub async fn for_group(pool: &PgPool, chat_id: i64) -> Result<StatsBundle> {
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '1 day'),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '1 day' AND deleted),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '1 day' AND banned),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '7 days'),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '7 days' AND deleted),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '7 days' AND banned),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '30 days'),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '30 days' AND deleted),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '30 days' AND banned),
            COUNT(*),
            COUNT(*) FILTER (WHERE deleted),
            COUNT(*) FILTER (WHERE banned)
        FROM detections
        WHERE chat_id = $1
        "#,
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await?;

    Ok(StatsBundle {
        day: PeriodStats {
            detected: row.0,
            deleted: row.1,
            banned: row.2,
        },
        week: PeriodStats {
            detected: row.3,
            deleted: row.4,
            banned: row.5,
        },
        month: PeriodStats {
            detected: row.6,
            deleted: row.7,
            banned: row.8,
        },
        all: PeriodStats {
            detected: row.9,
            deleted: row.10,
            banned: row.11,
        },
    })
}

/// The three headline numbers shown in `/start`.
pub async fn public_totals(pool: &PgPool) -> Result<(i64, i64, i64)> {
    let row: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM detections),
            (SELECT COUNT(*) FROM detections WHERE banned),
            (SELECT COUNT(*) FROM groups WHERE is_active)
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Everything `/info` reports.
pub async fn global(pool: &PgPool) -> Result<GlobalStats> {
    // `users` counts people who actually opened a private chat, not every
    // account ever seen in a group — otherwise the number is dominated by
    // passers-by and says nothing about the bot's reach.
    let reach: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM users WHERE started_bot),
            (SELECT COUNT(*) FROM users WHERE started_bot AND NOT is_blocked),
            (SELECT COUNT(*) FROM groups),
            (SELECT COUNT(*) FROM groups WHERE is_active),
            (SELECT COALESCE(SUM(member_count), 0) FROM groups WHERE is_active)
        "#,
    )
    .fetch_one(pool)
    .await?;

    let detections: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '1 day'),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '7 days'),
            COUNT(*) FILTER (WHERE created_at >= now() - INTERVAL '30 days'),
            COUNT(*)
        FROM detections
        "#,
    )
    .fetch_one(pool)
    .await?;

    let bans: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE banned_at >= now() - INTERVAL '1 day'),
            COUNT(*) FILTER (WHERE banned_at >= now() - INTERVAL '7 days'),
            COUNT(*) FILTER (WHERE banned_at >= now() - INTERVAL '30 days'),
            COUNT(*)
        FROM bans
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(GlobalStats {
        users: reach.0,
        active_users: reach.1,
        groups: reach.2,
        active_groups: reach.3,
        members: reach.4,
        detections: StatsBundle {
            day: PeriodStats {
                detected: detections.0,
                ..Default::default()
            },
            week: PeriodStats {
                detected: detections.1,
                ..Default::default()
            },
            month: PeriodStats {
                detected: detections.2,
                ..Default::default()
            },
            all: PeriodStats {
                detected: detections.3,
                ..Default::default()
            },
        },
        bans_day: bans.0,
        bans_week: bans.1,
        bans_month: bans.2,
        bans_all: bans.3,
    })
}

/// Groups with the most detections in the last 30 days, for `/info`.
pub async fn top_groups(pool: &PgPool, limit: i64) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
        r#"
        SELECT g.title, g.chat_id, COUNT(d.id) AS hits
        FROM groups g
        JOIN detections d ON d.chat_id = g.chat_id
        WHERE d.created_at >= now() - INTERVAL '30 days'
        GROUP BY g.chat_id, g.title
        ORDER BY hits DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(title, chat_id, hits)| (title.unwrap_or_else(|| chat_id.to_string()), hits))
        .collect())
}
