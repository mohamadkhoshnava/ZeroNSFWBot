//! Screens for the bot's own private chat.

use std::sync::Arc;

use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use super::{
    callbacks::{CallbackAction, PmView},
    panel::Screen,
};
use crate::{App, db, i18n::Lang, t, util::text::escape_html};

fn button(label: String, action: CallbackAction) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(label, action.encode())
}

pub async fn render(app: &Arc<App>, view: PmView, lang: Lang) -> Screen {
    match view {
        PmView::Start => start(app, lang).await,
        PmView::Help => help(app, lang),
        PmView::Test => test(lang),
        PmView::Lang => language(lang),
    }
}

/// The `/start` screen, including the running totals that show the bot is
/// actually doing something.
pub async fn start(app: &Arc<App>, lang: Lang) -> Screen {
    let (detections, bans, groups) = db::stats::public_totals(&app.db).await.unwrap_or_default();

    let text = format!(
        "{}\n{}\n\n{}\n\n{}",
        t!(lang, "start_title", bot = escape_html(&app.cfg.bot_name)),
        t!(lang, "start_body"),
        t!(
            lang,
            "start_global_stats",
            detections = detections,
            bans = bans,
            groups = groups
        ),
        t!(lang, "open_source", url = escape_html(&app.cfg.project_url)),
    ) + &app.cfg.credit_line(lang);

    let keyboard = InlineKeyboardMarkup::new(vec![
        // A url button rather than a callback: this is the one action that has
        // to hand the user off to Telegram's own chat picker.
        vec![InlineKeyboardButton::url(
            t!(lang, "btn_add_group"),
            add_to_group_url(app),
        )],
        vec![
            button(t!(lang, "btn_test"), CallbackAction::Pm(PmView::Test)),
            button(t!(lang, "btn_help"), CallbackAction::Pm(PmView::Help)),
        ],
        vec![
            button(t!(lang, "btn_language"), CallbackAction::Pm(PmView::Lang)),
            source_button(app, lang),
        ],
    ]);

    Screen { text, keyboard }
}

/// Parse a URL, falling back to `t.me` rather than panicking.
///
/// Both URLs here are built from environment variables, so a typo in
/// `BOT_USERNAME` or `PROJECT_URL` used to panic inside `/start` — taking down
/// the one screen every new user sees, on every press, until someone noticed.
fn url_or_fallback(raw: &str) -> reqwest::Url {
    raw.parse().unwrap_or_else(|_| {
        tracing::warn!(
            url = raw,
            "configured URL is not valid; using a placeholder"
        );
        "https://t.me".parse().expect("literal URL is valid")
    })
}

fn add_to_group_url(app: &Arc<App>) -> reqwest::Url {
    url_or_fallback(&app.cfg.add_to_group_url())
}

/// Link to the public repository.
fn source_button(app: &Arc<App>, lang: Lang) -> InlineKeyboardButton {
    InlineKeyboardButton::url(
        t!(lang, "btn_source"),
        url_or_fallback(&app.cfg.project_url),
    )
}

fn help(app: &Arc<App>, lang: Lang) -> Screen {
    Screen {
        text: format!(
            "{}\n\n{}",
            t!(lang, "help_body"),
            t!(lang, "open_source", url = escape_html(&app.cfg.project_url)),
        ) + &app.cfg.credit_line(lang),
        keyboard: InlineKeyboardMarkup::new(vec![vec![
            button(t!(lang, "btn_back"), CallbackAction::Pm(PmView::Start)),
            source_button(app, lang),
        ]]),
    }
}

fn test(lang: Lang) -> Screen {
    Screen {
        text: t!(lang, "test_prompt"),
        keyboard: InlineKeyboardMarkup::new(vec![vec![button(
            t!(lang, "btn_back"),
            CallbackAction::Pm(PmView::Start),
        )]]),
    }
}

fn language(lang: Lang) -> Screen {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Lang::ALL
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|option| {
                    let marker = if *option == lang { "✅" } else { "⬜️" };
                    button(
                        format!("{marker} {} {}", option.flag(), option.native_name()),
                        CallbackAction::SetUserLang(*option),
                    )
                })
                .collect()
        })
        .collect();

    rows.push(vec![button(
        t!(lang, "btn_back"),
        CallbackAction::Pm(PmView::Start),
    )]);

    Screen {
        text: t!(lang, "pm_lang_title"),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}
