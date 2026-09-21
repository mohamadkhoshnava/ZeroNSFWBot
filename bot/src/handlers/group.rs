//! Scanning of ordinary group messages.
//!
//! Everything here is designed to answer "should I even look at this?" as
//! cheaply as possible, because the answer is no for the overwhelming majority
//! of messages in an active group.

use std::sync::Arc;

use teloxide::types::Message;

use crate::{
    App, Tg, db, enforcement, filters, handlers::botguard, i18n, jev, media, policy, scan,
};

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

    // Checked before the sender filters below, and on its own terms: this is
    // about who was *added*, not about who sent the service message announcing
    // it. Telegram never delivers a bot's own messages to another bot, so an
    // arrival is the only moment a spam bot is visible to us at all.
    botguard::on_new_members(bot, msg, app).await?;

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

    let profile_pass = should_scan(app, &settings, chat_id, user_id, message_count).await;
    // The media scan deliberately ignores the grace window and the clean-user
    // cache. Both exist to answer "have we already established this account is
    // not a spam profile?", which says nothing about the picture in front of
    // us: a member of two years can still post pornography.
    let attachment = media::find(msg, &settings, app.cfg.media_max_bytes);

    // And neither does the text scan, for the same reason: what somebody
    // writes is judged on its own, not on how long they have been here.
    let message_text = msg.text().or_else(|| msg.caption()).unwrap_or_default();
    let text_pass = app.jev.enabled() && settings.scans_text() && !message_text.trim().is_empty();

    if !profile_pass && attachment.is_none() && !text_pass {
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
    // Whether the profile pass wants this same attachment. Only then is there a
    // second consumer for the score, and only then does its lower threshold
    // have any bearing on how carefully the score must be established.
    let shared = profile_pass && needs.message_media;

    // One score serves both passes, verified at whichever bar is lower, so a
    // score either of them would act on has always been through the second
    // stage — and the attachment is downloaded once, not twice.
    let media_scoring = match &attachment {
        Some(attachment) => {
            let verify_at = if shared {
                settings
                    .threshold_ratio()
                    .min(settings.media_threshold_ratio())
            } else {
                settings.media_threshold_ratio()
            };
            media::scan(app, bot, &settings, attachment, verify_at, user_id).await
        }
        None => None,
    };

    if let Some(scoring) = &media_scoring
        && scoring.score >= settings.media_threshold_ratio()
    {
        return media_verdict(app, bot, &settings, user, msg, scoring.clone()).await;
    }

    // One Jev call answers every topic the group chose *and* the advertising
    // question, because the model evaluates them in parallel against the same
    // state. Asking six things costs about what asking one costs.
    if text_pass
        && let Some(found) = jev::text::scan_cached(app, &settings, message_text).await
        && !found.is_empty()
    {
        return text_verdict(app, bot, &settings, user, msg, found).await;
    }

    if !profile_pass {
        return Ok(());
    }

    // Handed on only when the profile pass asked for message media. Feeding it
    // a signal its policy never requested would put a score in the report that
    // nothing consulted.
    let ctx = scan::collect(
        app,
        bot,
        settings.clone(),
        user,
        Some(msg),
        needs,
        shared.then_some(media_scoring).flatten(),
    )
    .await;

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

    enforcement::enforce(app, bot, &settings, &ctx, &report, &verdict, Some(msg.id)).await
}

/// Act on what a message says, on the text sections' own terms.
///
/// Built by hand for the same reason [`media_verdict`] is: the presets weigh an
/// avatar against a bio against a pinned channel, and none of that was looked
/// up here. The finding is about one message, so the verdict names the signals
/// that message produced and uses the sections' own actions.
///
/// A message can be both — an advert *and* off-topic — and it is dealt with
/// once, at the stronger of the two configured actions, with both reasons on
/// the report so an admin can see what it was judged for.
async fn text_verdict(
    app: &Arc<App>,
    bot: &Tg,
    settings: &crate::db::models::GroupSettings,
    user: &teloxide::types::User,
    msg: &Message,
    found: jev::text::TextVerdict,
) -> anyhow::Result<()> {
    let mut report = filters::ScanReport::default();
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut action = policy::Action::Report;
    let mut score = 0.0_f32;

    if let Some((topic, certainty)) = found.topic {
        report.insert(
            filters::F_TEXT_TOPIC,
            filters::FilterOutcome::triggered(
                certainty,
                // The topic's own name, untranslated, exactly as image scores
                // are stored: this is replayed in the details view long after
                // the scan, possibly in another language than the group had.
                Some(format!(
                    "{} {}%",
                    topic,
                    crate::util::text::percent(certainty)
                )),
            ),
        );
        reasons.push(filters::F_TEXT_TOPIC);
        action = action.strongest(settings.text_action);
        score = score.max(certainty);
    }

    if let Some(certainty) = found.advertising {
        report.insert(
            filters::F_TEXT_AD,
            filters::FilterOutcome::triggered(
                certainty,
                Some(format!("{}%", crate::util::text::percent(certainty))),
            ),
        );
        reasons.push(filters::F_TEXT_AD);
        action = action.strongest(settings.ad_action);
        score = score.max(certainty);
    }

    let verdict = policy::Verdict {
        matched: true,
        // Test mode is a promise that nothing will be changed, and it covers
        // every part of the bot or it is worthless.
        action: if settings.dry_run {
            policy::Action::Report
        } else {
            action
        },
        score,
        reasons,
    };

    // Nothing about the account was looked up, so every profile signal stays
    // unknown — `media_only` is the right shape for that even though what was
    // judged here was text.
    let ctx = scan::ScanContext::media_only(
        user.id.0 as i64,
        scan::display_name(user),
        user.username.clone(),
        settings.clone(),
        scan::ImageScoring::screened(0.0),
        app.cfg.global_reputation_min_bans,
    );

    enforcement::enforce(app, bot, settings, &ctx, &report, &verdict, Some(msg.id)).await
}

/// Act on explicit media, on the media section's own terms.
///
/// Built by hand rather than run through [`policy::evaluate`] on purpose. The
/// presets answer "does this account look like a spam profile", weighing an
/// avatar against a bio against a pinned channel. None of that is in evidence
/// here and none of it was looked up: the finding is about one picture, so the
/// verdict names one signal and uses the section's own action.
async fn media_verdict(
    app: &Arc<App>,
    bot: &Tg,
    settings: &crate::db::models::GroupSettings,
    user: &teloxide::types::User,
    msg: &Message,
    scoring: crate::scan::ImageScoring,
) -> anyhow::Result<()> {
    let mut report = filters::ScanReport::default();
    report.insert(
        filters::F_MESSAGE_MEDIA,
        filters::FilterOutcome::triggered(scoring.score, Some(scoring.detail())),
    );

    let verdict = policy::Verdict {
        matched: true,
        // Test mode is a promise that nothing will be changed, and it covers
        // every part of the bot or it is worthless.
        action: if settings.dry_run {
            policy::Action::Report
        } else {
            settings.media_action
        },
        score: scoring.score,
        reasons: vec![filters::F_MESSAGE_MEDIA],
    };

    let ctx = scan::ScanContext::media_only(
        user.id.0 as i64,
        scan::display_name(user),
        user.username.clone(),
        settings.clone(),
        scoring,
        app.cfg.global_reputation_min_bans,
    );

    enforcement::enforce(app, bot, settings, &ctx, &report, &verdict, Some(msg.id)).await
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
