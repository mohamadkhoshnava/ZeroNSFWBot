//! Scanning of ordinary group messages.
//!
//! Everything here is designed to answer "should I even look at this?" as
//! cheaply as possible, because the answer is no for the overwhelming majority
//! of messages in an active group.

use std::sync::Arc;

use teloxide::types::Message;

use crate::{App, Tg, db, enforcement, i18n, policy, scan};

pub async fn handle_message(bot: Tg, msg: Message, app: Arc<App>) -> anyhow::Result<()> {
    if let Err(err) = scan_message(&bot, &msg, &app).await {
        // One bad message must never stop the dispatcher.
        tracing::warn!(chat_id = msg.chat.id.0, %err, "group message handling failed");
    }
    Ok(())
}

async fn scan_message(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    if !(msg.chat.is_group() || msg.chat.is_supergroup()) {
        return Ok(());
    }

    // No `from` means an anonymous admin or a channel posting as itself —
    // neither is a spam account with a profile to scan.
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    if user.is_bot {
        return Ok(());
    }

    let chat_id = msg.chat.id.0;
    let user_id = user.id.0 as i64;

    let detected = i18n::detect_group_lang(
        msg.chat.title().unwrap_or_default(),
        None,
        app.cfg.defaults.lang,
    );
    let settings = db::groups::get_or_create(
        &app.db,
        chat_id,
        msg.chat.title(),
        &app.cfg.defaults,
        detected,
    )
    .await?;

    db::users::touch(
        &app.db,
        user_id,
        user.username.as_deref(),
        Some(&user.first_name),
        user.language_code
            .as_deref()
            .map_or(app.cfg.defaults.lang, i18n::Lang::from_telegram_code),
    )
    .await?;

    // Counting always happens, even when the scan is skipped: the grace window
    // is about how long someone has been present, not how often we looked.
    let message_count = db::groups::bump_message_count(&app.db, chat_id, user_id).await?;

    if !should_scan(app, &settings, chat_id, user_id, message_count).await {
        return Ok(());
    }

    // Admins are never scanned. They can already delete and ban; auto-banning
    // one would be both useless and spectacularly disruptive.
    let admins = app.admins.get(bot, msg.chat.id, app.bot_id).await;
    if admins.is_admin(user_id) || app.cfg.is_super_admin(user_id) {
        return Ok(());
    }

    let needs = app
        .filters
        .needs_for(&settings.policy.relevant_filters(&settings.custom_filters));

    let ctx = scan::collect(app, bot, settings.clone(), user, msg, needs).await;

    let report = app.filters.evaluate(&ctx).await;
    let verdict = policy::evaluate(
        &report,
        settings.policy,
        settings.action,
        &settings.custom_filters,
        settings.dry_run,
    );

    if !verdict.matched {
        // Remember the clean result so a chatty newcomer is not re-scanned on
        // every message for the rest of their grace window.
        app.recent_clean.insert((chat_id, user_id), ()).await;
        return Ok(());
    }

    enforcement::enforce(app, bot, &settings, &ctx, &report, &verdict, msg.id).await
}

/// The cheap pre-checks, ordered from cheapest to most expensive.
async fn should_scan(
    app: &Arc<App>,
    settings: &crate::db::models::GroupSettings,
    chat_id: i64,
    user_id: i64,
    message_count: i32,
) -> bool {
    // A grace window of 0 means "scan everyone, always".
    if settings.grace_messages > 0 && message_count > settings.grace_messages {
        return false;
    }

    if app.recent_clean.get(&(chat_id, user_id)).await.is_some() {
        return false;
    }

    true
}
