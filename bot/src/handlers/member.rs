//! `my_chat_member` — the bot's own membership changing.
//!
//! This is where a group is onboarded: the welcome message, the automatic
//! language guess, and the nudge to grant the permissions the bot needs.

use std::sync::Arc;

use serde_json::json;
use teloxide::{
    payloads::SendMessageSetters,
    prelude::*,
    types::{ChatMemberUpdated, InlineKeyboardButton, InlineKeyboardMarkup},
};

use crate::{
    App, Tg, db, i18n, t,
    ui::callbacks::{CallbackAction, PanelView},
    util::text::escape_html,
};

pub async fn handle_my_chat_member(
    bot: Tg,
    update: ChatMemberUpdated,
    app: Arc<App>,
) -> anyhow::Result<()> {
    let chat = &update.chat;
    if !(chat.is_group() || chat.is_supergroup()) {
        return Ok(());
    }

    let chat_id = chat.id.0;

    // The bot's rights changed, so any cached snapshot is now wrong.
    app.admins.invalidate(chat.id).await;

    if !update.new_chat_member.kind.is_present() {
        // Removed or banned: stop counting this group and stop broadcasting to it.
        db::groups::set_active(&app.db, chat_id, false).await?;
        db::audit::log(&app.db, Some(chat_id), None, "bot_removed", json!({})).await;
        tracing::info!(chat_id, "removed from group");
        return Ok(());
    }

    let was_present = update.old_chat_member.kind.is_present();

    // Description needs a separate call, and it materially improves the
    // language guess for groups with an emoji-only or Latin-branded title.
    let description = bot
        .get_chat(chat.id)
        .await
        .ok()
        .and_then(|full| full.description().map(str::to_owned));

    let detected = i18n::detect_group_lang(
        chat.title().unwrap_or_default(),
        description.as_deref(),
        app.cfg.defaults.lang,
    );

    let settings =
        db::groups::get_or_create(&app.db, chat_id, chat.title(), &app.cfg.defaults, detected)
            .await?;

    if let Ok(count) = bot.get_chat_member_count(chat.id).await {
        let _ = db::groups::set_member_count(&app.db, chat_id, count as i32).await;
    }

    // A promotion inside a group the bot already belonged to should not repeat
    // the welcome; only the first arrival gets it.
    if was_present {
        return Ok(());
    }

    db::audit::log(
        &app.db,
        Some(chat_id),
        None,
        "bot_added",
        json!({ "lang": settings.lang.code(), "title": chat.title() }),
    )
    .await;

    let lang = settings.lang;
    let mut text = format!(
        "{}\n{}",
        t!(lang, "welcome_title", bot = escape_html(&app.cfg.bot_name)),
        t!(
            lang,
            "welcome_body",
            threshold = settings.threshold,
            lang = lang.native_name()
        ),
    );

    let rights = update.new_chat_member.kind.clone();
    if !(rights.can_delete_messages() && rights.can_restrict_members()) {
        text.push_str("\n\n");
        text.push_str(&t!(lang, "welcome_admin_hint"));
    }

    // Only the welcome message carries the credit — putting it on every
    // detection report would turn each ban into an advertisement.
    text.push_str(&app.cfg.credit_line(lang));

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            t!(lang, "welcome_btn_settings"),
            CallbackAction::Panel(PanelView::Main).encode(),
        ),
        InlineKeyboardButton::callback(
            t!(lang, "welcome_btn_lang"),
            CallbackAction::Panel(PanelView::Lang).encode(),
        ),
    ]]);

    bot.send_message(chat.id, text)
        .reply_markup(keyboard)
        .await?;

    tracing::info!(chat_id, lang = lang.code(), "added to a new group");
    Ok(())
}
