use anyhow::Result;
use sqlx::PgPool;

use super::models::{GroupRow, GroupSettings};
use crate::{
    config::GroupDefaults,
    i18n::Lang,
    policy::{Action, Policy},
};

/// Expands to the column list `GroupRow` expects, in order.
///
/// A macro rather than a `const` so both queries stay `&'static str` literals,
/// which is what lets sqlx accept them without an injection-safety assertion.
macro_rules! group_columns {
    () => {
        "chat_id, title, username, lang, lang_locked, threshold, policy, \
         custom_filters, action, dry_run, grace_messages, delete_bot_messages, \
         bot_message_ttl_secs, global_blocklist, member_count, is_active, \
         added_at, updated_at"
    };
}

/// Fetch a group's settings, creating the row from the process defaults if the
/// bot has never seen this chat before.
pub async fn get_or_create(
    pool: &PgPool,
    chat_id: i64,
    title: Option<&str>,
    defaults: &GroupDefaults,
    detected_lang: Lang,
) -> Result<GroupSettings> {
    // A single round trip, and safe against two concurrent messages arriving
    // from the same brand-new group. The DO UPDATE branch touches only title
    // and is_active, so an existing group's settings are never overwritten.
    let row: GroupRow = sqlx::query_as(concat!(
        "INSERT INTO groups (chat_id, title, lang, threshold, policy, action, ",
        "                    dry_run, grace_messages, delete_bot_messages, ",
        "                    bot_message_ttl_secs) ",
        "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ",
        "ON CONFLICT (chat_id) DO UPDATE ",
        "    SET title      = COALESCE(EXCLUDED.title, groups.title), ",
        "        is_active  = TRUE, ",
        "        updated_at = now() ",
        "RETURNING ",
        group_columns!()
    ))
    .bind(chat_id)
    .bind(title)
    .bind(detected_lang.code())
    .bind(defaults.threshold)
    .bind(defaults.policy.as_str())
    .bind(defaults.action.as_str())
    .bind(defaults.dry_run)
    .bind(defaults.grace_messages)
    .bind(defaults.delete_bot_messages)
    .bind(defaults.bot_message_ttl_secs)
    .fetch_one(pool)
    .await?;

    Ok(row.into())
}

pub async fn get(pool: &PgPool, chat_id: i64) -> Result<Option<GroupSettings>> {
    let row: Option<GroupRow> = sqlx::query_as(concat!(
        "SELECT ",
        group_columns!(),
        " FROM groups WHERE chat_id = $1"
    ))
    .bind(chat_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

pub async fn set_lang(pool: &PgPool, chat_id: i64, lang: Lang) -> Result<()> {
    // lang_locked stops auto-detection from undoing a deliberate choice the
    // next time the group is renamed.
    sqlx::query(
        "UPDATE groups SET lang = $2, lang_locked = TRUE, updated_at = now() WHERE chat_id = $1",
    )
    .bind(chat_id)
    .bind(lang.code())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_threshold(pool: &PgPool, chat_id: i64, threshold: i16) -> Result<()> {
    sqlx::query("UPDATE groups SET threshold = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(threshold.clamp(0, 100))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_policy(pool: &PgPool, chat_id: i64, policy: Policy) -> Result<()> {
    sqlx::query("UPDATE groups SET policy = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(policy.as_str())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_custom_filters(pool: &PgPool, chat_id: i64, filters: &[String]) -> Result<()> {
    sqlx::query("UPDATE groups SET custom_filters = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(serde_json::to_value(filters)?)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_action(pool: &PgPool, chat_id: i64, action: Action) -> Result<()> {
    sqlx::query("UPDATE groups SET action = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(action.as_str())
        .execute(pool)
        .await?;
    Ok(())
}

/// Flip a boolean column and return its new value.
///
/// The column name is interpolated, which sqlx rightly makes you assert. It is
/// safe here because [`BoolColumn`] is a closed enum whose only `as_str` values
/// are the three hardcoded identifiers below — no caller can reach this format
/// string with arbitrary text.
pub async fn toggle_flag(pool: &PgPool, chat_id: i64, column: BoolColumn) -> Result<bool> {
    let sql = format!(
        "UPDATE groups SET {col} = NOT {col}, updated_at = now() \
         WHERE chat_id = $1 RETURNING {col}",
        col = column.as_str()
    );

    let value: (bool,) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(chat_id)
        .fetch_one(pool)
        .await?;
    Ok(value.0)
}

/// The boolean columns the settings panel can toggle. A closed enum rather than
/// a `&str` so no caller can ever reach the format string with arbitrary text.
#[derive(Debug, Clone, Copy)]
pub enum BoolColumn {
    DryRun,
    DeleteBotMessages,
    GlobalBlocklist,
}

impl BoolColumn {
    const fn as_str(self) -> &'static str {
        match self {
            BoolColumn::DryRun => "dry_run",
            BoolColumn::DeleteBotMessages => "delete_bot_messages",
            BoolColumn::GlobalBlocklist => "global_blocklist",
        }
    }
}

pub async fn set_grace(pool: &PgPool, chat_id: i64, messages: i32) -> Result<()> {
    sqlx::query("UPDATE groups SET grace_messages = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(messages.max(0))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn reset(pool: &PgPool, chat_id: i64, defaults: &GroupDefaults) -> Result<()> {
    // Language is intentionally preserved: it is not a moderation setting, and
    // resetting a Persian group to English would be a surprise.
    sqlx::query(
        r#"
        UPDATE groups
        SET threshold = $2, policy = $3, action = $4, custom_filters = '[]',
            dry_run = $5, grace_messages = $6, delete_bot_messages = $7,
            bot_message_ttl_secs = $8, global_blocklist = TRUE, updated_at = now()
        WHERE chat_id = $1
        "#,
    )
    .bind(chat_id)
    .bind(defaults.threshold)
    .bind(defaults.policy.as_str())
    .bind(defaults.action.as_str())
    .bind(defaults.dry_run)
    .bind(defaults.grace_messages)
    .bind(defaults.delete_bot_messages)
    .bind(defaults.bot_message_ttl_secs)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_active(pool: &PgPool, chat_id: i64, active: bool) -> Result<()> {
    sqlx::query("UPDATE groups SET is_active = $2, updated_at = now() WHERE chat_id = $1")
        .bind(chat_id)
        .bind(active)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_member_count(pool: &PgPool, chat_id: i64, count: i32) -> Result<()> {
    sqlx::query("UPDATE groups SET member_count = $2 WHERE chat_id = $1")
        .bind(chat_id)
        .bind(count)
        .execute(pool)
        .await?;
    Ok(())
}

/// Chat ids for a broadcast, newest groups last so failures cluster predictably.
pub async fn active_chat_ids(pool: &PgPool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT chat_id FROM groups WHERE is_active ORDER BY added_at")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Count a message toward the grace window and report the running total.
///
/// The returned value is the count *after* this message, so a `grace_messages`
/// of 5 means the sixth message onwards is exempt.
pub async fn bump_message_count(pool: &PgPool, chat_id: i64, user_id: i64) -> Result<i32> {
    let row: (i32,) = sqlx::query_as(
        r#"
        INSERT INTO group_members (chat_id, user_id, message_count)
        VALUES ($1, $2, 1)
        ON CONFLICT (chat_id, user_id)
            DO UPDATE SET message_count = group_members.message_count + 1
        RETURNING message_count
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Admins of this group who asked to be DM'd about removals *and* who have
/// started the bot, so the message can actually be delivered.
pub async fn notify_targets(pool: &PgPool, chat_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        SELECT p.admin_id
        FROM group_admin_prefs p
        JOIN users u ON u.user_id = p.admin_id
        WHERE p.chat_id = $1 AND p.dm_notify AND u.started_bot AND NOT u.is_blocked
        "#,
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn get_dm_notify(pool: &PgPool, chat_id: i64, admin_id: i64) -> Result<bool> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT dm_notify FROM group_admin_prefs WHERE chat_id = $1 AND admin_id = $2",
    )
    .bind(chat_id)
    .bind(admin_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some_and(|r| r.0))
}

pub async fn toggle_dm_notify(pool: &PgPool, chat_id: i64, admin_id: i64) -> Result<bool> {
    let row: (bool,) = sqlx::query_as(
        r#"
        INSERT INTO group_admin_prefs (chat_id, admin_id, dm_notify)
        VALUES ($1, $2, TRUE)
        ON CONFLICT (chat_id, admin_id)
            DO UPDATE SET dm_notify = NOT group_admin_prefs.dm_notify
        RETURNING dm_notify
        "#,
    )
    .bind(chat_id)
    .bind(admin_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}
