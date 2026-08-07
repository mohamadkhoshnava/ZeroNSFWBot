use anyhow::Result;
use sqlx::PgPool;

use super::models::UserRow;
use crate::i18n::Lang;

/// Expands to the column list `UserRow` expects. A macro so both queries stay
/// `&'static str` literals (see `groups::group_columns!`).
macro_rules! user_columns {
    () => {
        "user_id, username, first_name, lang, lang_locked, started_bot, is_blocked"
    };
}

/// Record a user we have seen, without claiming they started the bot.
///
/// Called for group participants so `/broadcast` and DM notifications know the
/// account exists, while `started_bot` stays false until they actually open a
/// private chat — the only state in which Telegram lets the bot message them.
pub async fn touch(
    pool: &PgPool,
    user_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    detected_lang: Lang,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO users (user_id, username, first_name, lang)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (user_id) DO UPDATE
            SET username   = EXCLUDED.username,
                first_name = EXCLUDED.first_name,
                last_seen  = now()
        "#,
    )
    .bind(user_id)
    .bind(username)
    .bind(first_name)
    .bind(detected_lang.code())
    .execute(pool)
    .await?;
    Ok(())
}

/// Called on `/start` in a private chat: from here on the bot may DM them.
pub async fn mark_started(
    pool: &PgPool,
    user_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    detected_lang: Lang,
) -> Result<UserRow> {
    // Telegram's reported language only wins until the user picks one
    // explicitly, hence the lang_locked guard in the UPDATE branch.
    let row: UserRow = sqlx::query_as(concat!(
        "INSERT INTO users (user_id, username, first_name, lang, started_bot) ",
        "VALUES ($1, $2, $3, $4, TRUE) ",
        "ON CONFLICT (user_id) DO UPDATE ",
        "    SET username    = EXCLUDED.username, ",
        "        first_name  = EXCLUDED.first_name, ",
        "        started_bot = TRUE, ",
        "        is_blocked  = FALSE, ",
        "        lang        = CASE WHEN users.lang_locked ",
        "                           THEN users.lang ELSE EXCLUDED.lang END, ",
        "        last_seen   = now() ",
        "RETURNING ",
        user_columns!()
    ))
    .bind(user_id)
    .bind(username)
    .bind(first_name)
    .bind(detected_lang.code())
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get(pool: &PgPool, user_id: i64) -> Result<Option<UserRow>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users WHERE user_id = $1"
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// The language to address this user in: their explicit choice if they made
/// one, otherwise what Telegram reports for their client.
pub async fn lang_for(pool: &PgPool, user_id: i64, telegram_code: Option<&str>) -> Lang {
    if let Ok(Some(row)) = get(pool, user_id).await
        && row.lang_locked
    {
        return row.lang();
    }

    telegram_code.map_or(Lang::En, Lang::from_telegram_code)
}

pub async fn set_lang(pool: &PgPool, user_id: i64, lang: Lang) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO users (user_id, lang, lang_locked)
        VALUES ($1, $2, TRUE)
        ON CONFLICT (user_id) DO UPDATE SET lang = EXCLUDED.lang, lang_locked = TRUE
        "#,
    )
    .bind(user_id)
    .bind(lang.code())
    .execute(pool)
    .await?;
    Ok(())
}

/// Called when Telegram reports the user blocked the bot, so broadcasts stop
/// wasting their rate-limit budget on a dead chat.
pub async fn mark_blocked(pool: &PgPool, user_id: i64) -> Result<()> {
    sqlx::query("UPDATE users SET is_blocked = TRUE WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn reachable_ids(pool: &PgPool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM users WHERE started_bot AND NOT is_blocked ORDER BY first_seen",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}
