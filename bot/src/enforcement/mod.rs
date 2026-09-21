//! Carries out a [`Verdict`] and reports what actually happened.
//!
//! Three rules shape the order of operations:
//!
//! * Moderation runs *before* the report, so the report can state the truth
//!   about whether the ban and deletion succeeded.
//! * The comment is deleted *before* the ban, and every claim about it comes
//!   from that call's own result. A ban with `revoke_messages` usually removes
//!   the comment too, but "usually" is not something a report may assert.
//! * The report replies to the offending comment only when that comment still
//!   exists — i.e. when deletion failed. Replying to a message the bot just
//!   deleted would leave a dangling quote.

pub mod ephemeral;

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use teloxide::{
    ApiError, RequestError,
    payloads::SendMessageSetters,
    prelude::*,
    types::{
        ChatId, ChatPermissions, InlineKeyboardButton, InlineKeyboardMarkup, MessageId,
        ReplyParameters, UserId,
    },
};

use crate::{
    App, Tg,
    db::{
        self,
        models::{GroupSettings, NewDetection, StoredReason},
    },
    filters::{F_TEXT_AD, F_TEXT_TOPIC, ScanReport},
    i18n::Lang,
    policy::{Action, Verdict},
    scan::ScanContext,
    t,
    ui::callbacks::CallbackAction,
    util::text::{escape_html, percent, truncate},
};

/// How long a muted user stays muted. Long enough to stop a spam run, short
/// enough that a false positive is not a life sentence.
const MUTE_HOURS: i64 = 24;

/// What the bot managed to do, as opposed to what it intended to do.
#[derive(Debug, Default, Clone)]
struct Executed {
    banned: bool,
    deleted: bool,
    muted: bool,
    /// Whether there was a message to act on at all. False for a detection
    /// triggered by a reaction, where the only thing in the chat belongs to
    /// somebody innocent.
    had_message: bool,
    /// Translation keys for permission problems, shown verbatim in the report.
    problems: Vec<&'static str>,
}

impl Executed {
    /// True when there is an offending message still in the chat for the report
    /// to reply to.
    fn message_survived(&self) -> bool {
        self.had_message && !self.deleted
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn enforce(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    ctx: &ScanContext,
    report: &ScanReport,
    verdict: &Verdict,
    // The message that triggered this, when there is one. `None` for a
    // reaction: the message under it was written by somebody else, and
    // deleting it — or replying to it as though it were the offence — would
    // punish the wrong person.
    message_id: Option<MessageId>,
) -> anyhow::Result<()> {
    let chat_id = ChatId(settings.chat_id);
    let user_id = ctx.user_id;
    let lang = settings.lang;

    let admins = app.admins.get(bot, chat_id, app.bot_id).await;
    let executed = execute(bot, chat_id, user_id, message_id, verdict.action, &admins).await;

    let reasons: Vec<StoredReason> = verdict
        .reasons
        .iter()
        .map(|id| StoredReason {
            filter: (*id).to_owned(),
            score: report.score(id),
            detail: report.get(id).and_then(|o| o.detail.clone()),
        })
        .collect();

    let detection_id = db::detections::insert(
        &app.db,
        &NewDetection {
            chat_id: settings.chat_id,
            user_id,
            message_id: message_id.map(|id| id.0),
            score: verdict.score,
            verdict: verdict.action,
            banned: executed.banned,
            deleted: executed.deleted,
            muted: executed.muted,
            dry_run: settings.dry_run,
            reasons: reasons.clone(),
        },
    )
    .await?;

    if executed.banned {
        db::bans::record(&app.db, settings.chat_id, user_id, verdict.score, &reasons).await?;
    }

    db::audit::log(
        &app.db,
        Some(settings.chat_id),
        None,
        "detection",
        json!({
            "user_id": user_id,
            "score": verdict.score,
            "action": verdict.action.as_str(),
            "banned": executed.banned,
            "deleted": executed.deleted,
            "dry_run": settings.dry_run,
            "reasons": verdict.reasons,
        }),
    )
    .await;

    post_report(
        bot,
        settings,
        ctx,
        verdict,
        &executed,
        message_id,
        detection_id,
        lang,
    )
    .await;

    notify_admins(app, bot, settings, ctx, verdict, &executed).await;
    notify_offender(app, bot, settings, ctx, verdict, &executed).await;

    Ok(())
}

/// Perform the moderation calls, recording both successes and the reason for
/// each failure.
async fn execute(
    bot: &Tg,
    chat_id: ChatId,
    user_id: i64,
    message_id: Option<MessageId>,
    action: Action,
    admins: &crate::util::admin_cache::AdminSnapshot,
) -> Executed {
    let mut done = Executed {
        had_message: message_id.is_some(),
        ..Executed::default()
    };

    // Neither changes anything in the chat. A warning is delivered by the
    // report itself, which is posted as a reply to the message — so it must
    // leave that message exactly where it is.
    if matches!(action, Action::Report | Action::Warn) {
        return done;
    }

    if !admins.bot_is_admin {
        // Nothing below can succeed; say so once rather than three times.
        done.problems.push("missing_admin");
        return done;
    }

    // Delete before banning, and from the real result of the call.
    //
    // Banning with revoke_messages usually takes the comment with it, so this
    // used to just assume the comment was gone and never call deleteMessage at
    // all. When the sweep does not reach the comment — it is not guaranteed,
    // least of all for channel comments — the comment stayed in the group while
    // the report, the admin DM and the database all recorded it as deleted.
    //
    // Doing it first is also the only way to get a usable answer: after a ban,
    // "message to delete not found" cannot distinguish a successful revoke from
    // a comment that was never there.
    if let Some(message_id) = message_id.filter(|_| admins.bot_can_delete) {
        match bot.delete_message(chat_id, message_id).await {
            Ok(_) => done.deleted = true,
            // Already gone — the sender removed it, or another admin did.
            Err(RequestError::Api(ApiError::MessageToDeleteNotFound)) => done.deleted = true,
            Err(err) => {
                tracing::warn!(%chat_id, %err, "delete failed");
                done.problems.push("missing_delete_perm");
            }
        }
    } else if done.had_message {
        done.problems.push("missing_delete_perm");
    }

    let wants_ban = action == Action::Ban;
    let wants_mute = action == Action::Mute;

    if wants_ban {
        if admins.bot_can_restrict {
            // revoke_messages sweeps the rest of the spam run in one call —
            // these accounts rarely stop at a single comment.
            match bot
                .ban_chat_member(chat_id, UserId(user_id as u64))
                .revoke_messages(true)
                .await
            {
                Ok(_) => done.banned = true,
                Err(err) => {
                    tracing::warn!(%chat_id, user_id, %err, "ban failed");
                    done.problems.push("missing_ban_perm");
                }
            }
        } else {
            done.problems.push("missing_ban_perm");
        }
    }

    if wants_mute {
        if admins.bot_can_restrict {
            let until = Utc::now() + chrono::Duration::hours(MUTE_HOURS);
            match bot
                .restrict_chat_member(chat_id, UserId(user_id as u64), ChatPermissions::empty())
                .until_date(until)
                .await
            {
                Ok(_) => done.muted = true,
                Err(err) => {
                    tracing::warn!(%chat_id, user_id, %err, "mute failed");
                    done.problems.push("missing_ban_perm");
                }
            }
        } else {
            done.problems.push("missing_ban_perm");
        }
    }

    done.problems.dedup();
    done
}

/// Post the public report in the group.
#[allow(clippy::too_many_arguments)]
async fn post_report(
    bot: &Tg,
    settings: &GroupSettings,
    ctx: &ScanContext,
    verdict: &Verdict,
    executed: &Executed,
    message_id: Option<MessageId>,
    detection_id: i64,
    lang: Lang,
) {
    let user = escape_html(&truncate(&ctx.display_name, 48));

    let headline = t!(
        lang,
        headline_key(settings.dry_run, verdict.action, executed),
        user = user
    );

    let mut text = format!(
        "{headline}\n{}\n{}",
        t!(
            lang,
            score_key(&verdict.reasons),
            score = percent(verdict.score)
        ),
        t!(
            lang,
            "report_reasons",
            reasons = reason_list(lang, &verdict.reasons)
        ),
    );

    for problem in &executed.problems {
        text.push_str(&t!(lang, "report_failed", problem = t!(lang, problem)));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            t!(lang, "btn_mark_wrong"),
            CallbackAction::FalsePositive(detection_id).encode(),
        ),
        InlineKeyboardButton::callback(
            t!(lang, "btn_details"),
            CallbackAction::Details(detection_id).encode(),
        ),
    ]]);

    let mut request = bot
        .send_message(ChatId(settings.chat_id), text)
        .reply_markup(keyboard);

    // Only quote the comment if it is still there to quote. For a warning this
    // is the whole delivery mechanism: the reply is what puts the notice under
    // the message it is about, where the person who sent it will see it.
    if let Some(message_id) = message_id.filter(|_| executed.message_survived()) {
        request = request
            .reply_parameters(ReplyParameters::new(message_id).allow_sending_without_reply());
    }

    match request.await {
        Ok(sent) if settings.delete_bot_messages => {
            ephemeral::schedule_delete(
                bot.clone(),
                ChatId(settings.chat_id),
                sent.id,
                settings.bot_message_ttl_secs,
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(chat_id = settings.chat_id, %err, "could not post the report"),
    }
}

/// Pick the report headline for what actually happened.
///
/// Whether the comment survived is part of the outcome, not a detail: an admin
/// reading "comment removed and X banned" stops looking for the comment. Every
/// punishment therefore has a `_kept` variant for the case where the account
/// was dealt with but the comment is still in the group.
fn headline_key(dry_run: bool, action: Action, executed: &Executed) -> &'static str {
    if dry_run {
        return "report_dry_run";
    }
    // A warning changes nothing on purpose, so it must not fall through to
    // "I could not act" — which is the same outcome for entirely the opposite
    // reason, and reads as the bot being broken.
    if action == Action::Warn {
        return "report_warned";
    }
    match (executed.banned, executed.muted, executed.deleted) {
        (true, _, true) => "report_banned",
        (true, _, false) => "report_banned_kept",
        (false, true, true) => "report_muted",
        (false, true, false) => "report_muted_kept",
        (false, false, true) => "report_deleted_only",
        // Detected, but every action failed — usually missing permissions. The
        // problems list carries the specifics; the headline must not claim test
        // mode is on when it is not.
        (false, false, false) => "report_no_action",
    }
}

/// Whether everything this verdict rests on came from reading the message.
///
/// The headline number means different things in the two cases, and calling
/// both of them "NSFW probability" is simply wrong: a message removed for
/// being 88% political has nothing to do with nudity, and an admin reading
/// that line would have no idea what the bot actually objected to.
fn text_only(reasons: &[&'static str]) -> bool {
    !reasons.is_empty()
        && reasons
            .iter()
            .all(|id| *id == F_TEXT_TOPIC || *id == F_TEXT_AD)
}

fn score_key(reasons: &[&'static str]) -> &'static str {
    if text_only(reasons) {
        "report_score_text"
    } else {
        "report_score"
    }
}

/// Translate the filter ids into a human-readable list.
fn reason_list(lang: Lang, reasons: &[&'static str]) -> String {
    reasons
        .iter()
        .map(|id| t!(lang, &format!("reason_{id}")))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// DM the admins who opted in via `/nsfw → My DM alerts`.
async fn notify_admins(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    ctx: &ScanContext,
    verdict: &Verdict,
    executed: &Executed,
) {
    if settings.dry_run || !(executed.deleted || executed.banned || executed.muted) {
        return;
    }

    let targets = match db::groups::notify_targets(&app.db, settings.chat_id).await {
        Ok(targets) => targets,
        Err(err) => {
            tracing::warn!(%err, "could not load DM notification targets");
            return;
        }
    };

    // The DM used to state the comment was removed no matter what happened,
    // which is how an admin ends up being told a comment is gone while it is
    // still sitting in the group.
    let key = match (text_only(&verdict.reasons), executed.deleted) {
        (false, true) => "dm_notify",
        (false, false) => "dm_notify_kept",
        (true, true) => "dm_notify_text",
        (true, false) => "dm_notify_text_kept",
    };

    for admin_id in targets {
        let lang = db::users::lang_for_group_notice(&app.db, admin_id, settings.lang).await;
        let text = t!(
            lang,
            key,
            chat = escape_html(settings.title.as_deref().unwrap_or("—")),
            user = escape_html(&truncate(&ctx.display_name, 48)),
            score = percent(verdict.score),
            reasons = reason_list(lang, &verdict.reasons),
        );

        if let Err(err) = bot.send_message(ChatId(admin_id), text).await {
            handle_dm_failure(app, admin_id, &err).await;
        }
    }
}

/// Tell the removed user why, and offer an appeal.
///
/// Best-effort by nature: Telegram only allows this if they have ever started
/// the bot, which most spam accounts have not.
async fn notify_offender(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    ctx: &ScanContext,
    verdict: &Verdict,
    executed: &Executed,
) {
    if !executed.banned || settings.dry_run {
        return;
    }

    let lang = db::users::lang_for_group_notice(&app.db, ctx.user_id, settings.lang).await;

    // Telling someone their *profile* was flagged when what happened is that
    // their GIF was would send them to change their avatar over nothing, and
    // makes the appeal harder to argue.
    let key = if verdict.reasons == [crate::filters::F_MESSAGE_MEDIA] {
        "banned_notice_media"
    } else if text_only(&verdict.reasons) {
        "banned_notice_text"
    } else {
        "banned_notice"
    };

    let text = t!(
        lang,
        key,
        chat = escape_html(settings.title.as_deref().unwrap_or("—")),
        score = percent(verdict.score),
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        t!(lang, "btn_appeal"),
        CallbackAction::Appeal(settings.chat_id).encode(),
    )]]);

    // A failure here is the normal case, not an error worth logging loudly.
    let _ = bot
        .send_message(ChatId(ctx.user_id), text)
        .reply_markup(keyboard)
        .await;
}

/// Mark a user as unreachable when Telegram says they blocked the bot, so
/// broadcasts and future notifications skip them.
pub async fn handle_dm_failure(app: &Arc<App>, user_id: i64, err: &RequestError) {
    let blocked = matches!(
        err,
        RequestError::Api(ApiError::BotBlocked | ApiError::UserDeactivated)
    );

    if blocked && let Err(err) = db::users::mark_blocked(&app.db, user_id).await {
        tracing::warn!(%err, user_id, "could not mark user as blocked");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executed(banned: bool, muted: bool, deleted: bool) -> Executed {
        Executed {
            banned,
            deleted,
            muted,
            had_message: true,
            problems: Vec::new(),
        }
    }

    /// The outcome of a reaction-triggered detection: an account was dealt
    /// with, and there was never a message of theirs to remove.
    fn without_message(banned: bool) -> Executed {
        Executed {
            banned,
            had_message: false,
            ..Executed::default()
        }
    }

    fn headline(executed: &Executed) -> &'static str {
        headline_key(false, Action::Ban, executed)
    }

    /// The regression this whole change exists for: a ban whose message sweep
    /// missed the comment was reported as "comment removed and X banned", so
    /// admins were told a comment was gone while it was still in the group.
    #[test]
    fn a_ban_that_left_the_comment_says_so() {
        assert_eq!(
            headline(&executed(true, false, false)),
            "report_banned_kept"
        );
        assert_eq!(headline(&executed(true, false, true)), "report_banned");
    }

    #[test]
    fn a_mute_that_left_the_comment_says_so() {
        assert_eq!(headline(&executed(false, true, false)), "report_muted_kept");
        assert_eq!(headline(&executed(false, true, true)), "report_muted");
    }

    #[test]
    fn no_headline_claims_a_deletion_that_did_not_happen() {
        for (banned, muted) in [(false, false), (true, false), (false, true)] {
            let key = headline(&executed(banned, muted, false));
            assert!(
                !matches!(
                    key,
                    "report_banned" | "report_muted" | "report_deleted_only"
                ),
                "{key} claims the comment was removed when it was not"
            );
        }
    }

    #[test]
    fn a_deletion_on_its_own_is_reported_as_such() {
        assert_eq!(
            headline(&executed(false, false, true)),
            "report_deleted_only"
        );
    }

    #[test]
    fn nothing_done_never_looks_like_test_mode() {
        assert_eq!(headline(&executed(false, false, false)), "report_no_action");
        assert_eq!(
            headline_key(true, Action::Ban, &executed(false, false, false)),
            "report_dry_run"
        );
    }

    /// Dry run reports what *would* have happened, so it must win over any
    /// outcome flags that leaked through.
    #[test]
    fn dry_run_always_wins() {
        assert_eq!(
            headline_key(true, Action::Ban, &executed(true, false, true)),
            "report_dry_run"
        );
    }

    /// A warning deliberately changes nothing, which must not be reported with
    /// the headline that means "I tried and failed".
    #[test]
    fn a_warning_is_not_reported_as_a_failure() {
        assert_eq!(
            headline_key(false, Action::Warn, &executed(false, false, false)),
            "report_warned"
        );
    }

    /// Test mode still wins over a warning: nothing was changed either way, but
    /// the group is being told what *would* have happened.
    #[test]
    fn dry_run_wins_over_a_warning_too() {
        assert_eq!(
            headline_key(true, Action::Warn, &executed(false, false, false)),
            "report_dry_run"
        );
    }

    /// The report's headline number is labelled by what produced it. A
    /// message removed for being political is not an NSFW probability.
    #[test]
    fn a_text_only_detection_does_not_report_an_nsfw_probability() {
        assert_eq!(score_key(&[F_TEXT_TOPIC]), "report_score_text");
        assert_eq!(score_key(&[F_TEXT_TOPIC, F_TEXT_AD]), "report_score_text");

        assert_eq!(score_key(&[crate::filters::F_PROFILE_NSFW]), "report_score");
        // One image signal among text ones is still an image score.
        assert_eq!(
            score_key(&[F_TEXT_AD, crate::filters::F_MESSAGE_MEDIA]),
            "report_score"
        );
        // And an empty list must not claim to be a text finding.
        assert_eq!(score_key(&[]), "report_score");
    }

    /// A reaction-triggered ban has no message of the offender's to quote. The
    /// message under the reaction belongs to somebody else, and replying to it
    /// would tell that person they had been banned.
    #[test]
    fn a_detection_with_no_message_never_replies_to_one() {
        assert!(!without_message(true).message_survived());
        assert!(!without_message(false).message_survived());
    }

    #[test]
    fn the_report_replies_only_to_a_comment_that_still_exists() {
        assert!(executed(true, false, false).message_survived());
        assert!(!executed(true, false, true).message_survived());
    }
}
