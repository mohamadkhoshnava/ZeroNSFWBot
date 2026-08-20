//! Banning the bots nobody promoted.
//!
//! A spam bot in a group defeats every filter this project has. Its avatar is a
//! logo, its bio is empty, its name is ordinary, and its messages are plain
//! text advertising — there is no NSFW signal to find, so no threshold and no
//! policy will ever fire. What is reliably true instead is structural: an
//! automation a group actually wanted is one an admin promoted. Everything else
//! was dropped in by whoever could click "add member".
//!
//! So the rule is deliberately blunt and has nothing to do with scanning:
//! **a bot that is not an administrator here does not belong here.** It is off
//! by default, because plenty of groups run an unpromoted helper bot quite
//! happily and inheriting a rule that removes it would be a nasty surprise.
//!
//! ## Where this can and cannot see
//!
//! Telegram never delivers one bot's messages to another, so waiting for a spam
//! bot to *post* is not an option — by the time it speaks, the update is
//! already not ours to receive. Both entry points here are therefore about
//! arrival, not behaviour:
//!
//! * [`on_new_members`] — the `new_chat_members` service message, which is sent
//!   by the *human* who did the adding and so does reach us.
//! * [`on_chat_member`] — the membership update, which covers a bot that let
//!   itself in through an invite link. It costs an extra `allowed_updates`
//!   entry, which teloxide derives from the handler tree.
//!
//! Neither can enumerate the bots that were already sitting in the group when
//! the switch was turned on; Telegram has no "list members" call for that.

use std::sync::Arc;

use serde_json::json;
use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberUpdated, Message, User, UserId},
};

use crate::{
    App, Tg, db, enforcement::ephemeral, i18n::Lang, t, util::admin_cache::AdminSnapshot,
    util::text::escape_html,
};

/// Bots added by a human, from the `new_chat_members` service message.
pub async fn on_new_members(bot: &Tg, msg: &Message, app: &Arc<App>) -> anyhow::Result<()> {
    let Some(members) = msg.new_chat_members() else {
        return Ok(());
    };
    if !members.iter().any(|user| user.is_bot) {
        return Ok(());
    }

    let settings = match db::groups::get(&app.db, msg.chat.id.0).await? {
        Some(settings) => settings,
        // No row yet means the bot has never been configured here; the group
        // handler creates it on the first ordinary message.
        None => return Ok(()),
    };

    for member in members.iter().filter(|user| user.is_bot) {
        remove(bot, app, &settings, msg.chat.id, member).await;
    }

    Ok(())
}

/// A bot whose membership changed — the invite-link path, where there is no
/// service message naming who added it.
pub async fn on_chat_member(
    bot: Tg,
    update: ChatMemberUpdated,
    app: Arc<App>,
) -> anyhow::Result<()> {
    let chat = &update.chat;
    if !(chat.is_group() || chat.is_supergroup()) {
        return Ok(());
    }

    let member = &update.new_chat_member.user;
    if !member.is_bot {
        return Ok(());
    }

    // Only an arrival is interesting. A bot that was already present and simply
    // had its title changed is not news, and re-banning on every such update
    // would be a loop.
    if update.old_chat_member.kind.is_present() || !update.new_chat_member.kind.is_present() {
        return Ok(());
    }

    let Some(settings) = db::groups::get(&app.db, chat.id.0).await? else {
        return Ok(());
    };

    remove(&bot, &app, &settings, chat.id, member).await;
    Ok(())
}

/// Ban one bot, if this group asked for that and this bot is not exempt.
///
/// Every early return here is a case where banning would be wrong, so they are
/// checked before the ban rather than compensated for after it.
async fn remove(
    bot: &Tg,
    app: &Arc<App>,
    settings: &crate::db::models::GroupSettings,
    chat_id: ChatId,
    member: &User,
) {
    let user_id = member.id.0 as i64;

    if !settings.ban_foreign_bots {
        return;
    }
    // Cheap exits before the admin lookup, which is an API call on a cache miss.
    if user_id == app.bot_id {
        return;
    }

    let admins = app.admins.get(bot, chat_id, app.bot_id).await;

    match verdict(user_id, app.bot_id, &admins) {
        Verdict::Leave => return,
        Verdict::Powerless => {
            tracing::warn!(
                chat_id = chat_id.0,
                user_id,
                "bot guard is on but I cannot restrict members here"
            );
            return;
        }
        Verdict::Ban => {}
    }

    // revoke_messages sweeps whatever it already managed to post, which for an
    // advertising bot is the entire reason it was added.
    match bot
        .ban_chat_member(chat_id, UserId(user_id as u64))
        .revoke_messages(true)
        .await
    {
        Ok(_) => {
            tracing::info!(
                chat_id = chat_id.0,
                user_id,
                username = member.username.as_deref().unwrap_or("—"),
                "banned a bot nobody promoted"
            );
            db::audit::log(
                &app.db,
                Some(chat_id.0),
                None,
                "foreign_bot_banned",
                json!({ "user_id": user_id, "username": member.username }),
            )
            .await;
            announce(bot, settings, chat_id, member).await;
        }
        Err(err) => {
            tracing::warn!(chat_id = chat_id.0, user_id, %err, "could not ban a bot");
        }
    }
}

/// What to do about one bot that just arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ban,
    /// Exempt, and deliberately so — see [`verdict`].
    Leave,
    /// Would be banned, but the bot has no right to restrict anyone here.
    Powerless,
}

/// Whether this bot should be removed, as a pure function.
///
/// Extracted from [`remove`] so the exemptions can be tested without a
/// Telegram client: every one of them is a case where banning would be an
/// outright bug — banning ourselves would make the group unmanageable, and
/// banning a promoted bot would delete the very automation an admin chose.
pub fn verdict(user_id: i64, bot_id: i64, admins: &AdminSnapshot) -> Verdict {
    if user_id == bot_id {
        return Verdict::Leave;
    }
    // The whole point of the rule: a promoted bot is a wanted bot. Checked
    // against both lists because a bot promoted here is filed under
    // `admin_bot_ids`, while `admin_ids` guards against a future change to how
    // the snapshot classifies members.
    if admins.is_admin_bot(user_id) || admins.is_admin(user_id) {
        return Verdict::Leave;
    }
    if !admins.bot_can_restrict {
        return Verdict::Powerless;
    }
    Verdict::Ban
}

/// Say what happened, so admins do not discover the rule by noticing an absence.
async fn announce(
    bot: &Tg,
    settings: &crate::db::models::GroupSettings,
    chat_id: ChatId,
    member: &User,
) {
    let lang: Lang = settings.lang;
    let name = member
        .username
        .as_ref()
        .map(|u| format!("@{u}"))
        .unwrap_or_else(|| member.first_name.clone());

    let text = t!(lang, "botguard_banned", bot = escape_html(&name));

    match bot.send_message(chat_id, text).await {
        Ok(sent) if settings.delete_bot_messages => {
            ephemeral::schedule_delete(
                bot.clone(),
                chat_id,
                sent.id,
                settings.bot_message_ttl_secs,
            );
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(chat_id = chat_id.0, %err, "could not announce a bot ban"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bot itself, an admin bot, and a plain bot in one snapshot.
    fn snapshot(can_restrict: bool) -> AdminSnapshot {
        AdminSnapshot {
            admin_ids: vec![100],
            admin_bot_ids: vec![200],
            bot_is_admin: true,
            bot_can_delete: true,
            bot_can_restrict: can_restrict,
        }
    }

    const ME: i64 = 999;

    #[test]
    fn an_unpromoted_bot_is_banned() {
        assert_eq!(verdict(555, ME, &snapshot(true)), Verdict::Ban);
    }

    /// The exemption the whole feature is built around: promoting a bot is how
    /// an admin says they want it.
    #[test]
    fn a_promoted_bot_is_left_alone() {
        assert_eq!(verdict(200, ME, &snapshot(true)), Verdict::Leave);
    }

    /// The regression that would be unrecoverable: banning ourselves takes the
    /// settings panel with it, so no admin could turn the rule back off.
    #[test]
    fn i_never_ban_myself() {
        assert_eq!(verdict(ME, ME, &snapshot(true)), Verdict::Leave);
    }

    /// Belt and braces: an id that somehow reached `admin_ids` is an admin
    /// whatever list it landed in.
    #[test]
    fn an_id_in_the_human_admin_list_is_still_exempt() {
        assert_eq!(verdict(100, ME, &snapshot(true)), Verdict::Leave);
    }

    /// Without the right to restrict, the ban would fail anyway — reported as
    /// its own verdict so the caller can say so rather than logging a Telegram
    /// error nobody can act on.
    #[test]
    fn no_restrict_right_is_reported_separately() {
        assert_eq!(verdict(555, ME, &snapshot(false)), Verdict::Powerless);
    }
}
