//! Non-command messages in the bot's private chat.
//!
//! The only thing accepted here is a photo for the detector test, so people can
//! see what the model actually thinks before trusting it with their group.

use std::sync::Arc;

use teloxide::{net::Download, prelude::*, types::Message};

use crate::{App, Tg, db, detector::ImageItem, t, util::text::percent};

pub async fn handle_message(bot: Tg, msg: Message, app: Arc<App>) -> anyhow::Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let user_id = user.id.0 as i64;
    let lang = db::users::lang_for(&app.db, user_id, user.language_code.as_deref()).await;

    let Some(photo) = msg
        .photo()
        .and_then(|sizes| sizes.iter().max_by_key(|p| p.width * p.height))
    else {
        // Anything that is not a photo gets the menu rather than silence.
        let screen = crate::ui::private::start(&app, lang).await;
        bot.send_message(msg.chat.id, screen.text)
            .reply_markup(screen.keyboard)
            .await?;
        return Ok(());
    };

    if let Err(retry_after) = app.test_limiter.check(user_id).await {
        bot.send_message(
            msg.chat.id,
            t!(
                lang,
                "test_rate_limited",
                seconds = retry_after.as_secs().max(1)
            ),
        )
        .await?;
        return Ok(());
    }

    let score = classify(&bot, &app, photo).await;

    let text = match score {
        Some(nsfw) => {
            let threshold = app.cfg.defaults.threshold;
            let verdict = if percent(nsfw) >= threshold as u8 {
                t!(lang, "test_verdict_flagged", threshold = threshold)
            } else {
                t!(lang, "test_verdict_clean", threshold = threshold)
            };
            t!(
                lang,
                "test_result",
                nsfw = percent(nsfw),
                sfw = 100 - percent(nsfw),
                verdict = verdict
            )
        }
        None => t!(lang, "test_failed"),
    };

    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

/// Score one photo. Nothing is written to the database: the test is a
/// demonstration, and storing strangers' uploads would be the opposite of the
/// privacy promise in `/help`.
async fn classify(bot: &Tg, app: &Arc<App>, photo: &teloxide::types::PhotoSize) -> Option<f32> {
    let file = bot.get_file(photo.file.id.clone()).await.ok()?;
    let mut bytes = Vec::with_capacity(photo.file.size as usize);
    bot.download_file(&file.path, &mut bytes).await.ok()?;

    let item = ImageItem::new(photo.file.unique_id.0.clone(), &bytes);
    app.detector
        .classify(std::slice::from_ref(&item))
        .await
        .inspect_err(|err| tracing::warn!(%err, "test-mode classification failed"))
        .ok()?
        .into_values()
        .next()
}
