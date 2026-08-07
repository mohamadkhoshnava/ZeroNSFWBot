//! Slash commands.
//!
//! The access rules the bot is built around live here: in a group it answers
//! `/nsfw` only for administrators, and it silently deletes the command when an
//! ordinary member sends it — no reply, so the bot cannot be used as a noise
//! generator by whoever spams the command.

use std::sync::Arc;

use teloxide::{
    payloads::SendMessageSetters,
    prelude::*,
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Me, Message},
    utils::command::BotCommands,
};

use crate::{
    App, Tg, admin,
    admin::broadcast::{self, Pending, Target},
    db,
    i18n::Lang,
    t,
    ui::{
        callbacks::{CallbackAction, PanelView, PmView},
        panel, private,
    },
    util::text::{escape_html, truncate},
};

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    /// Introduction and settings in private; ignored in groups.
    Start,
    /// How detection works.
    Help,
    /// Open the moderation settings for this group.
    Nsfw,
    /// Bot-wide status. Super-admins only.
    Info,
    /// Announce something to every user or group. Super-admins only.
    #[command(parse_with = "default")]
    Broadcast(String),
}

pub async fn handle(
    bot: Tg,
    msg: Message,
    cmd: Command,
    app: Arc<App>,
    me: Me,
) -> anyhow::Result<()> {
    let _ = me;
    match cmd {
        Command::Start => start(&bot, &msg, &app).await,
        Command::Help => help(&bot, &msg, &app).await,
        Command::Nsfw => nsfw(&bot, &msg, &app).await,
        Command::Info => info(&bot, &msg, &app).await,
        Command::Broadcast(args) => broadcast_cmd(&bot, &msg, &app, &args).await,
    }
}

/// Language for whoever sent this message in a private chat.
async fn pm_lang(app: &Arc<App>, msg: &Message) -> Lang {
    let Some(user) = msg.from.as_ref() else {
        return app.cfg.defaults.lang;
    };
    db::users::lang_for(&app.db, user.id.0 as i64, user.language_code.as_deref()).await
}

async fn start(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    // In a group /start is noise; the panel lives behind /nsfw.
    if !msg.chat.is_private() {
        return Ok(());
    }

    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };

    let detected = user
        .language_code
        .as_deref()
        .map_or(app.cfg.defaults.lang, Lang::from_telegram_code);

    let row = db::users::mark_started(
        &app.db,
        user.id.0 as i64,
        user.username.as_deref(),
        Some(&user.first_name),
        detected,
    )
    .await?;

    let screen = private::start(app, row.lang()).await;
    bot.send_message(msg.chat.id, screen.text)
        .reply_markup(screen.keyboard)
        .await?;
    Ok(())
}

async fn help(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    if !msg.chat.is_private() {
        return Ok(());
    }

    let lang = pm_lang(app, msg).await;
    let screen = private::render(app, PmView::Help, lang).await;
    bot.send_message(msg.chat.id, screen.text)
        .reply_markup(screen.keyboard)
        .await?;
    Ok(())
}

/// `/nsfw` — the settings panel.
async fn nsfw(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    if msg.chat.is_private() {
        // There is no group to configure from here, so point at the one thing
        // that helps: adding the bot somewhere.
        let lang = pm_lang(app, msg).await;
        let screen = private::start(app, lang).await;
        bot.send_message(msg.chat.id, screen.text)
            .reply_markup(screen.keyboard)
            .await?;
        return Ok(());
    }

    let Some(user) = msg.from.as_ref() else {
        // Anonymous admins post as the chat itself and carry no user id, so
        // there is nobody to authorise.
        return Ok(());
    };

    let user_id = user.id.0 as i64;
    let admins = app.admins.get(bot, msg.chat.id, app.bot_id).await;

    if !admins.is_admin(user_id) && !app.cfg.is_super_admin(user_id) {
        // Silently clean up: replying would let anyone make the bot talk.
        if admins.bot_can_delete {
            let _ = bot.delete_message(msg.chat.id, msg.id).await;
        }
        return Ok(());
    }

    let detected = crate::i18n::detect_group_lang(
        msg.chat.title().unwrap_or_default(),
        None,
        app.cfg.defaults.lang,
    );
    let settings = db::groups::get_or_create(
        &app.db,
        msg.chat.id.0,
        msg.chat.title(),
        &app.cfg.defaults,
        detected,
    )
    .await?;

    let screen = panel::render(app, PanelView::Main, &settings, user_id).await;
    let mut text = screen.text;

    // Say it here rather than only at the first detection: an admin opening the
    // panel is exactly the person who can fix missing permissions.
    if !(admins.bot_is_admin && admins.bot_can_delete && admins.bot_can_restrict) {
        text.push_str("\n\n");
        text.push_str(&t!(settings.lang, "welcome_admin_hint"));
    }

    bot.send_message(msg.chat.id, text)
        .reply_markup(screen.keyboard)
        .await?;
    Ok(())
}

async fn info(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let lang = pm_lang(app, msg).await;

    if !app.cfg.is_super_admin(user.id.0 as i64) {
        // Only answer in private; in a group this would just be noise.
        if msg.chat.is_private() {
            bot.send_message(msg.chat.id, t!(lang, "not_authorized"))
                .await?;
        }
        return Ok(());
    }

    bot.send_message(msg.chat.id, admin::info::render(app, lang).await?)
        .await?;
    Ok(())
}

/// `/broadcast <target> <text>`, or reply to a message with `/broadcast <target>`.
///
/// Always renders a preview with an explicit confirm button; nothing is sent
/// from this handler.
async fn broadcast_cmd(bot: &Tg, msg: &Message, app: &Arc<App>, args: &str) -> anyhow::Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let admin_id = user.id.0 as i64;
    let lang = pm_lang(app, msg).await;

    if !app.cfg.is_super_admin(admin_id) {
        if msg.chat.is_private() {
            bot.send_message(msg.chat.id, t!(lang, "not_authorized"))
                .await?;
        }
        return Ok(());
    }

    if app.broadcasts.is_running() {
        bot.send_message(msg.chat.id, t!(lang, "broadcast_busy"))
            .await?;
        return Ok(());
    }

    let (target_word, rest) = args
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or((args.trim(), ""));
    let Some(target) = Target::parse(target_word) else {
        bot.send_message(msg.chat.id, t!(lang, "broadcast_usage"))
            .await?;
        return Ok(());
    };

    // The body is either the rest of the command or the replied-to message.
    let body = if rest.trim().is_empty() {
        msg.reply_to_message()
            .and_then(|reply| reply.text().or_else(|| reply.caption()))
            .unwrap_or_default()
            .to_owned()
    } else {
        rest.trim().to_owned()
    };

    if body.trim().is_empty() {
        bot.send_message(msg.chat.id, t!(lang, "broadcast_usage"))
            .await?;
        return Ok(());
    }

    // Echo the body verbatim, exactly as recipients will receive it. This
    // doubles as validation: broadcasts are sent with HTML parse mode, so
    // malformed markup would otherwise fail on every one of N sends instead of
    // once, here, where it can still be fixed.
    if let Err(err) = bot.send_message(ChatId(admin_id), &body).await {
        bot.send_message(
            ChatId(admin_id),
            t!(
                lang,
                "broadcast_bad_html",
                error = escape_html(&truncate(&err.to_string(), 200))
            ),
        )
        .await?;
        return Ok(());
    }

    let recipients = broadcast::recipients(app, target).await?;
    let count = recipients.len();

    app.broadcasts
        .stage(
            admin_id,
            Pending {
                target,
                body,
                recipients,
            },
        )
        .await;

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            t!(lang, "broadcast_confirm"),
            CallbackAction::BroadcastConfirm.encode(),
        ),
        InlineKeyboardButton::callback(
            t!(lang, "btn_cancel"),
            CallbackAction::BroadcastCancel.encode(),
        ),
    ]]);

    bot.send_message(
        ChatId(admin_id),
        t!(
            lang,
            "broadcast_preview",
            target = t!(lang, target.label_key()),
            count = count,
        ),
    )
    .reply_markup(keyboard)
    .await?;

    Ok(())
}
