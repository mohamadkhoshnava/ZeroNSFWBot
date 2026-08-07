//! Inline-button handling.
//!
//! Authorisation is re-checked on every press rather than trusted from whoever
//! opened the panel: keyboards persist in chat history, so a message posted for
//! an admin can be pressed by anyone scrolling past it later.

use std::sync::Arc;

use serde_json::json;
use teloxide::{
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters, SendMessageSetters},
    prelude::*,
    types::{CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, UserId},
};

use crate::{
    App, Tg,
    admin::broadcast,
    db,
    db::models::GroupSettings,
    i18n::Lang,
    t,
    ui::{
        Screen,
        callbacks::{CallbackAction, PanelView, PmView},
        panel, private,
    },
    util::text::escape_html,
};

pub async fn handle(bot: Tg, query: CallbackQuery, app: Arc<App>) -> anyhow::Result<()> {
    let Some(action) = query.data.as_deref().and_then(CallbackAction::decode) else {
        // Stale keyboard from an older version, or someone poking at the API.
        let _ = bot.answer_callback_query(query.id.clone()).await;
        return Ok(());
    };

    if let Err(err) = dispatch(&bot, &query, &app, action).await {
        tracing::warn!(%err, "callback handling failed");
        let lang = user_lang(&app, &query).await;
        let _ = bot
            .answer_callback_query(query.id.clone())
            .text(t!(lang, "error_generic"))
            .await;
    }

    Ok(())
}

async fn dispatch(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    action: CallbackAction,
) -> anyhow::Result<()> {
    use CallbackAction::*;

    match action {
        Pm(view) => pm_view(bot, query, app, view).await,
        SetUserLang(lang) => set_user_lang(bot, query, app, lang).await,

        FalsePositive(id) => false_positive(bot, query, app, id).await,
        Details(id) => details(bot, query, app, id).await,

        Appeal(chat_id) => appeal(bot, query, app, chat_id).await,
        AppealResolve {
            chat_id,
            user_id,
            approve,
        } => appeal_resolve(bot, query, app, chat_id, user_id, approve).await,

        BroadcastConfirm => broadcast_confirm(bot, query, app).await,
        BroadcastCancel => broadcast_cancel(bot, query, app).await,

        // Everything else is a group settings action.
        other => group_setting(bot, query, app, other).await,
    }
}

// --------------------------------------------------------------- helpers ---

async fn user_lang(app: &Arc<App>, query: &CallbackQuery) -> Lang {
    db::users::lang_for(
        &app.db,
        query.from.id.0 as i64,
        query.from.language_code.as_deref(),
    )
    .await
}

/// The chat and message the pressed keyboard belongs to.
///
/// `None` when Telegram considers the message inaccessible (too old, or the
/// bot was removed), in which case there is nothing to edit.
fn origin(query: &CallbackQuery) -> Option<(ChatId, MessageId)> {
    let message = query.message.as_ref()?;
    Some((message.chat().id, message.id()))
}

/// Replace the message the button belongs to with a freshly rendered screen.
async fn swap(bot: &Tg, query: &CallbackQuery, screen: Screen) -> anyhow::Result<()> {
    let Some((chat_id, message_id)) = origin(query) else {
        return Ok(());
    };

    // Telegram rejects an edit that changes nothing; pressing an already-active
    // option is a normal thing to do, so that error is expected and ignored.
    let _ = bot
        .edit_message_text(chat_id, message_id, screen.text)
        .reply_markup(screen.keyboard)
        .await;
    Ok(())
}

// ------------------------------------------------------------ private chat --

async fn pm_view(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    view: PmView,
) -> anyhow::Result<()> {
    let lang = user_lang(app, query).await;
    bot.answer_callback_query(query.id.clone()).await?;
    swap(bot, query, private::render(app, view, lang).await).await
}

async fn set_user_lang(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    lang: Lang,
) -> anyhow::Result<()> {
    db::users::set_lang(&app.db, query.from.id.0 as i64, lang).await?;

    bot.answer_callback_query(query.id.clone())
        .text(t!(lang, "lang_changed", value = lang.native_name()))
        .await?;

    swap(bot, query, private::render(app, PmView::Lang, lang).await).await
}

// ------------------------------------------------------------ group panel --

/// Apply a settings change, then re-render.
///
/// All group actions funnel through here so the admin check exists in exactly
/// one place.
async fn group_setting(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    action: CallbackAction,
) -> anyhow::Result<()> {
    use CallbackAction::*;

    let Some((chat_id, message_id)) = origin(query) else {
        return Ok(());
    };
    let admin_id = query.from.id.0 as i64;

    let Some(settings) = db::groups::get(&app.db, chat_id.0).await? else {
        let _ = bot.answer_callback_query(query.id.clone()).await;
        return Ok(());
    };

    let admins = app.admins.get(bot, chat_id, app.bot_id).await;
    if !admins.is_admin(admin_id) && !app.cfg.is_super_admin(admin_id) {
        bot.answer_callback_query(query.id.clone())
            .text(t!(settings.lang, "only_admins"))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    if matches!(action, Close) {
        bot.answer_callback_query(query.id.clone()).await?;
        let _ = bot.delete_message(chat_id, message_id).await;
        return Ok(());
    }

    let view = apply(app, &settings, admin_id, action.clone()).await?;

    // A detection-affecting change must apply to the next message, not
    // whenever the clean-user cache happens to expire. Admins tune the
    // threshold by watching what happens next; a 15-minute lag would read as
    // the setting not working.
    if affects_detection(&action) {
        let chat = chat_id.0;
        let _ = app
            .recent_clean
            .invalidate_entries_if(move |(cached_chat, _), _| *cached_chat == chat);
    }

    bot.answer_callback_query(query.id.clone())
        .text(t!(settings.lang, "saved"))
        .await?;

    // Re-read: `apply` wrote to the database, and the panel must show the
    // stored truth rather than an optimistic guess.
    let updated = db::groups::get(&app.db, chat_id.0)
        .await?
        .unwrap_or(settings);

    swap(
        bot,
        query,
        panel::render(app, view, &updated, admin_id).await,
    )
    .await
}

/// Does this change alter who would be flagged?
///
/// Cosmetic settings (language, DM alerts, statistics) do not, so they leave
/// the clean-user cache alone.
fn affects_detection(action: &CallbackAction) -> bool {
    use CallbackAction::*;
    matches!(
        action,
        AdjustThreshold(_)
            | SetPolicy(_)
            | ToggleCustom(_)
            | SetAction(_)
            | ToggleDryRun
            | ToggleGlobal
            | SetGrace(_)
            | Reset
    )
}

/// Perform one settings mutation and return the screen to show next.
async fn apply(
    app: &Arc<App>,
    settings: &GroupSettings,
    admin_id: i64,
    action: CallbackAction,
) -> anyhow::Result<PanelView> {
    use CallbackAction::*;

    let chat_id = settings.chat_id;

    Ok(match action {
        Panel(view) => view,

        AdjustThreshold(delta) => {
            let next = (settings.threshold + delta).clamp(0, 100);
            db::groups::set_threshold(&app.db, chat_id, next).await?;
            PanelView::Threshold
        }

        SetPolicy(policy) => {
            db::groups::set_policy(&app.db, chat_id, policy).await?;
            // Choosing "custom" without filters selects nothing, so go straight
            // to the editor instead of leaving a policy that never matches.
            if policy == crate::policy::Policy::Custom && settings.custom_filters.is_empty() {
                PanelView::Custom
            } else {
                PanelView::Policy
            }
        }

        ToggleCustom(filter) => {
            let mut filters = settings.custom_filters.clone();
            match filters.iter().position(|f| *f == filter) {
                Some(index) => {
                    filters.remove(index);
                }
                None => filters.push(filter),
            }
            db::groups::set_custom_filters(&app.db, chat_id, &filters).await?;
            PanelView::Custom
        }

        SetAction(value) => {
            db::groups::set_action(&app.db, chat_id, value).await?;
            PanelView::Action
        }

        SetGroupLang(lang) => {
            db::groups::set_lang(&app.db, chat_id, lang).await?;
            PanelView::Lang
        }

        ToggleNotify => {
            db::groups::toggle_dm_notify(&app.db, chat_id, admin_id).await?;
            PanelView::Notify
        }

        ToggleDryRun => {
            db::groups::toggle_flag(&app.db, chat_id, db::groups::BoolColumn::DryRun).await?;
            PanelView::DryRun
        }

        ToggleGlobal => {
            db::groups::toggle_flag(&app.db, chat_id, db::groups::BoolColumn::GlobalBlocklist)
                .await?;
            PanelView::Global
        }

        ToggleAutoDelete => {
            db::groups::toggle_flag(&app.db, chat_id, db::groups::BoolColumn::DeleteBotMessages)
                .await?;
            PanelView::AutoDelete
        }

        SetGrace(value) => {
            db::groups::set_grace(&app.db, chat_id, value).await?;
            PanelView::Grace
        }

        Reset => {
            db::groups::reset(&app.db, chat_id, &app.cfg.defaults).await?;
            db::audit::log(
                &app.db,
                Some(chat_id),
                Some(admin_id),
                "settings_reset",
                json!({}),
            )
            .await;
            PanelView::Main
        }

        // Non-settings actions never reach here.
        _ => PanelView::Main,
    })
}

// -------------------------------------------------------------- detections --

/// An admin marks a detection as wrong: undo the ban and record the mistake.
async fn false_positive(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    detection_id: i64,
) -> anyhow::Result<()> {
    let admin_id = query.from.id.0 as i64;

    let Some(detection) = db::detections::get(&app.db, detection_id).await? else {
        let lang = user_lang(app, query).await;
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "detection_expired"))
            .await?;
        return Ok(());
    };

    let chat_id = ChatId(detection.chat_id);
    let lang = db::groups::get(&app.db, detection.chat_id)
        .await?
        .map_or(Lang::En, |s| s.lang);

    let admins = app.admins.get(bot, chat_id, app.bot_id).await;
    if !admins.is_admin(admin_id) && !app.cfg.is_super_admin(admin_id) {
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "only_admins"))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    db::detections::mark_incorrect(&app.db, detection_id).await?;

    let unbanned = if detection.banned {
        let result = bot
            .unban_chat_member(chat_id, UserId(detection.user_id as u64))
            .await;
        if result.is_ok() {
            db::bans::record_unban(&app.db, detection.chat_id, detection.user_id, admin_id).await?;
        }
        result.is_ok()
    } else {
        true
    };

    db::audit::log(
        &app.db,
        Some(detection.chat_id),
        Some(admin_id),
        "false_positive",
        json!({ "detection_id": detection_id, "user_id": detection.user_id, "unbanned": unbanned }),
    )
    .await;

    bot.answer_callback_query(query.id.clone())
        .text(t!(
            lang,
            if unbanned {
                "marked_wrong"
            } else {
                "marked_wrong_failed"
            }
        ))
        .show_alert(true)
        .await?;

    Ok(())
}

/// Show which filters fired and what they saw.
async fn details(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    detection_id: i64,
) -> anyhow::Result<()> {
    let Some(detection) = db::detections::get(&app.db, detection_id).await? else {
        let lang = user_lang(app, query).await;
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "detection_expired"))
            .await?;
        return Ok(());
    };

    // The details belong to a group's report, so use that group's language.
    let lang = db::groups::get(&app.db, detection.chat_id)
        .await?
        .map_or(Lang::En, |s| s.lang);

    let mut text = t!(lang, "details_title");
    for reason in detection.reasons() {
        let value = reason
            .detail
            .unwrap_or_else(|| format!("{}%", (reason.score * 100.0).round() as i32));
        text.push('\n');
        text.push_str(&t!(
            lang,
            "details_line",
            filter = escape_html(&reason.filter),
            value = value
        ));
    }

    // An alert rather than a new message: details are for the admin who asked,
    // not another line of bot output in the group.
    bot.answer_callback_query(query.id.clone())
        .text(strip_html(&text))
        .show_alert(true)
        .await?;

    Ok(())
}

/// Callback alerts are plain text, so the HTML the message templates carry has
/// to come back out.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

// ----------------------------------------------------------------- appeals --

async fn appeal(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    chat_id: i64,
) -> anyhow::Result<()> {
    let user_id = query.from.id.0 as i64;
    let lang = user_lang(app, query).await;

    let opened = db::appeals::open(&app.db, chat_id, user_id).await?;
    if !opened {
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "appeal_already"))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    bot.answer_callback_query(query.id.clone())
        .text(t!(lang, "appeal_sent"))
        .show_alert(true)
        .await?;

    // Route it to the admins who already asked to hear about this group.
    let settings = db::groups::get(&app.db, chat_id).await?;
    let group_lang = settings.as_ref().map_or(Lang::En, |s| s.lang);
    let title = settings
        .as_ref()
        .and_then(|s| s.title.clone())
        .unwrap_or_else(|| chat_id.to_string());

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            t!(group_lang, "btn_appeal_approve"),
            CallbackAction::AppealResolve {
                chat_id,
                user_id,
                approve: true,
            }
            .encode(),
        ),
        InlineKeyboardButton::callback(
            t!(group_lang, "btn_appeal_reject"),
            CallbackAction::AppealResolve {
                chat_id,
                user_id,
                approve: false,
            }
            .encode(),
        ),
    ]]);

    let notice = t!(
        group_lang,
        "appeal_admin_notice",
        user = escape_html(&query.from.first_name),
        chat = escape_html(&title),
    );

    for admin_id in db::groups::notify_targets(&app.db, chat_id).await? {
        let _ = bot
            .send_message(ChatId(admin_id), notice.clone())
            .reply_markup(keyboard.clone())
            .await;
    }

    Ok(())
}

async fn appeal_resolve(
    bot: &Tg,
    query: &CallbackQuery,
    app: &Arc<App>,
    chat_id: i64,
    user_id: i64,
    approve: bool,
) -> anyhow::Result<()> {
    let admin_id = query.from.id.0 as i64;
    let lang = db::groups::get(&app.db, chat_id)
        .await?
        .map_or(Lang::En, |s| s.lang);

    let admins = app.admins.get(bot, ChatId(chat_id), app.bot_id).await;
    if !admins.is_admin(admin_id) && !app.cfg.is_super_admin(admin_id) {
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "only_admins"))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    db::appeals::resolve(&app.db, chat_id, user_id, approve, admin_id).await?;

    if approve {
        let _ = bot
            .unban_chat_member(ChatId(chat_id), UserId(user_id as u64))
            .await;
        db::bans::record_unban(&app.db, chat_id, user_id, admin_id).await?;
    }

    db::audit::log(
        &app.db,
        Some(chat_id),
        Some(admin_id),
        "appeal_resolved",
        json!({ "user_id": user_id, "approved": approve }),
    )
    .await;

    bot.answer_callback_query(query.id.clone())
        .text(t!(
            lang,
            if approve {
                "appeal_approved"
            } else {
                "appeal_rejected"
            }
        ))
        .show_alert(true)
        .await?;

    Ok(())
}

// -------------------------------------------------------------- broadcasts --

async fn broadcast_confirm(bot: &Tg, query: &CallbackQuery, app: &Arc<App>) -> anyhow::Result<()> {
    let admin_id = query.from.id.0 as i64;
    let lang = user_lang(app, query).await;

    if !app.cfg.is_super_admin(admin_id) {
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "not_authorized"))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    let Some(pending) = app.broadcasts.take(admin_id).await else {
        bot.answer_callback_query(query.id.clone())
            .text(t!(lang, "broadcast_cancelled"))
            .await?;
        return Ok(());
    };

    bot.answer_callback_query(query.id.clone()).await?;
    if let Some((chat_id, message_id)) = origin(query) {
        let _ = bot.delete_message(chat_id, message_id).await;
    }

    // Detached: a broadcast to thousands of chats takes minutes, and the
    // dispatcher must keep serving everyone else meanwhile.
    let (app, bot) = (app.clone(), bot.clone());
    tokio::spawn(async move {
        if let Err(err) = broadcast::run(app, bot, admin_id, lang, pending).await {
            tracing::error!(%err, "broadcast failed");
        }
    });

    Ok(())
}

async fn broadcast_cancel(bot: &Tg, query: &CallbackQuery, app: &Arc<App>) -> anyhow::Result<()> {
    let admin_id = query.from.id.0 as i64;
    let lang = user_lang(app, query).await;

    app.broadcasts.discard(admin_id).await;
    bot.answer_callback_query(query.id.clone())
        .text(t!(lang, "broadcast_cancelled"))
        .await?;

    if let Some((chat_id, message_id)) = origin(query) {
        let _ = bot.delete_message(chat_id, message_id).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_html;

    #[test]
    fn alerts_are_plain_text() {
        assert_eq!(
            strip_html("<b>Details</b>\n• <code>bio_link</code> — t.me/x"),
            "Details\n• bio_link — t.me/x"
        );
    }

    #[test]
    fn entities_are_decoded_back() {
        assert_eq!(strip_html("a &amp; b &lt;c&gt;"), "a & b <c>");
    }
}
