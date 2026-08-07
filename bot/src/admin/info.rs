//! `/info` — the bot-wide status report for super-admins.

use std::sync::Arc;

use anyhow::Result;

use crate::{
    App, db,
    i18n::Lang,
    t,
    util::text::{escape_html, humanize_duration, truncate},
};

pub async fn render(app: &Arc<App>, lang: Lang) -> Result<String> {
    let stats = db::stats::global(&app.db).await?;

    // A failing detector is the single most important thing this report can
    // surface: without it every scan silently degrades to text-only signals.
    let detector = match app.detector.health().await {
        Ok(health) if health.model_loaded => format!("✅ {}", health.status),
        Ok(health) => format!("⚠️ {}", health.status),
        Err(err) => format!("❌ {}", escape_html(&truncate(&err.to_string(), 80))),
    };

    let mut text = format!(
        "{}\n{}",
        t!(lang, "info_title", bot = escape_html(&app.cfg.bot_name)),
        t!(
            lang,
            "info_body",
            users = stats.users,
            active_users = stats.active_users,
            groups = stats.groups,
            active_groups = stats.active_groups,
            members = stats.members,
            d_day = stats.detections.day.detected,
            d_week = stats.detections.week.detected,
            d_month = stats.detections.month.detected,
            d_all = stats.detections.all.detected,
            b_day = stats.bans_day,
            b_week = stats.bans_week,
            b_month = stats.bans_month,
            b_all = stats.bans_all,
            detector = detector,
            uptime = humanize_duration(app.uptime_secs()),
        ),
    );

    if let Ok(top) = db::stats::top_groups(&app.db, 5).await
        && !top.is_empty()
    {
        let rows = top
            .iter()
            .map(|(title, hits)| format!("• {} — {hits}", escape_html(&truncate(title, 32))))
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str(&t!(lang, "info_top_groups", rows = rows));
    }

    Ok(text)
}
