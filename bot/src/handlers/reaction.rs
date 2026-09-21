//! Scanning the accounts that react, not just the ones that write.
//!
//! Reaction spam is what comment spam turns into once comments are being
//! removed. The account taps an emoji on somebody else's message; its avatar
//! and display name appear under that message for everyone who opens the
//! reaction list, and there is nothing of its own in the chat to delete. Every
//! signal the bot already has — the avatar, the bio, the attached channel —
//! applies unchanged. All that was missing was pointing the scan at reactors.
//!
//! Two things make this different from scanning a message:
//!
//! * **Nothing is deleted.** The message under the reaction belongs to somebody
//!   innocent, so the verdict carries no message id at all and
//!   [`crate::enforcement`] skips the deletion and the reply. Banning the
//!   account removes its reaction with it.
//! * **No message is counted.** A reaction must not advance the grace-window
//!   counter, or an account could react its way past ever being scanned.
//!
//! Telegram only delivers `message_reaction` updates to a bot that is an
//! administrator of the chat, so a group that has not promoted the bot sees
//! nothing here — which is also the case in which the bot could not act anyway.

use std::sync::Arc;

use teloxide::types::MessageReactionUpdated;

use crate::{App, Tg, db, enforcement, policy, scan};

pub async fn handle(bot: Tg, update: MessageReactionUpdated, app: Arc<App>) -> anyhow::Result<()> {
    if let Err(err) = scan_reaction(&bot, &update, &app).await {
        tracing::warn!(chat_id = update.chat.id.0, %err, "reaction handling failed");
    }
    Ok(())
}

async fn scan_reaction(
    bot: &Tg,
    update: &MessageReactionUpdated,
    app: &Arc<App>,
) -> anyhow::Result<()> {
    let chat = &update.chat;
    if !(chat.is_group() || chat.is_supergroup()) {
        return Ok(());
    }

    // An anonymous admin or a channel reacting on its own behalf carries no
    // user profile to scan.
    let Some(user) = update.user() else {
        return Ok(());
    };
    if user.is_bot {
        return Ok(());
    }

    // Removing a reaction is not an act of advertising. Only an addition puts
    // the account's avatar in front of the group.
    if update.new_reaction.is_empty() {
        return Ok(());
    }

    // Deliberately `get` rather than `get_or_create`: a group the bot has
    // never seen has no settings, and a reaction is not the event that should
    // onboard one.
    let Some(settings) = db::groups::get(&app.db, chat.id.0).await? else {
        return Ok(());
    };
    if !settings.reaction_scan {
        return Ok(());
    }

    let chat_id = chat.id.0;
    let user_id = user.id.0 as i64;

    // The same two cheap gates the message path uses — read, never advanced.
    // An account that has only ever reacted has a count of zero, which is
    // exactly the newcomer the grace window is meant to cover.
    if settings.grace_messages > 0 {
        let count = db::groups::message_count(&app.db, chat_id, user_id).await?;
        if count > settings.grace_messages {
            return Ok(());
        }
    }
    if app.recent_clean.get(&(chat_id, user_id)).await.is_some() {
        return Ok(());
    }

    let admins = app.admins.get(bot, chat.id, app.bot_id).await;
    if admins.is_admin(user_id) || app.cfg.is_super_admin(user_id) {
        return Ok(());
    }

    let needs = app
        .filters
        .needs_for(&settings.policy.relevant_filters(&settings.custom_filters));

    let ctx = scan::collect(app, bot, settings.clone(), user, None, needs, None).await;
    let report = app.filters.evaluate(&ctx).await;
    let verdict = policy::evaluate(
        &report,
        settings.policy,
        settings.action,
        &settings.custom_filters,
        settings.dry_run,
    );

    if !verdict.matched {
        app.recent_clean.insert((chat_id, user_id), ()).await;
        return Ok(());
    }

    tracing::info!(
        chat_id,
        user_id,
        score = verdict.score,
        "acting on a reaction from a flagged account"
    );

    // No message id: the only message in sight is the one they reacted *to*,
    // and it is not theirs.
    enforcement::enforce(app, bot, &settings, &ctx, &report, &verdict, None).await
}
