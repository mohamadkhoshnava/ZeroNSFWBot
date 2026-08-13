//! The `/nsfw` settings panel.
//!
//! Every screen is regenerated from the stored [`GroupSettings`] rather than
//! from any in-memory wizard state, so two admins editing at once always see
//! the truth, and a restart never strands a half-finished dialogue.

use std::sync::Arc;

use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use super::callbacks::{CallbackAction, PanelView};
use crate::{
    App,
    db::{self, models::GroupSettings},
    filters::ALL_FILTERS,
    i18n::Lang,
    policy::{Action, Policy},
    t,
    util::text::escape_html,
};

/// A rendered screen: message text plus its keyboard.
pub struct Screen {
    pub text: String,
    pub keyboard: InlineKeyboardMarkup,
}

fn button(label: String, action: CallbackAction) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(label, action.encode())
}

fn back_row(lang: Lang) -> Vec<InlineKeyboardButton> {
    vec![
        button(t!(lang, "btn_back"), CallbackAction::Panel(PanelView::Main)),
        button(t!(lang, "btn_close"), CallbackAction::Close),
    ]
}

fn on_off(lang: Lang, value: bool) -> String {
    t!(lang, if value { "state_on" } else { "state_off" })
}

/// Prefix a label with its current state, so a toggle's effect is obvious
/// before it is pressed.
fn checkbox(selected: bool, label: String) -> String {
    format!("{} {label}", if selected { "✅" } else { "⬜️" })
}

fn grace_label(lang: Lang, grace: i32) -> String {
    if grace <= 0 {
        t!(lang, "grace_off")
    } else {
        t!(lang, "grace_value", count = grace)
    }
}

pub async fn render(
    app: &Arc<App>,
    view: PanelView,
    settings: &GroupSettings,
    admin_id: i64,
) -> Screen {
    let lang = settings.lang;

    match view {
        PanelView::Main => main(app, settings, lang),
        PanelView::Threshold => threshold(settings, lang),
        PanelView::Policy => policy(settings, lang),
        PanelView::Custom => custom(settings, lang),
        PanelView::Categories => categories(app, settings, lang),
        PanelView::Action => action(settings, lang),
        PanelView::Lang => language(settings, lang),
        PanelView::Notify => notify(app, settings, admin_id, lang).await,
        PanelView::Stats => stats(app, settings, lang).await,
        PanelView::DryRun => simple_toggle(
            lang,
            t!(lang, "dryrun_title"),
            settings.dry_run,
            CallbackAction::ToggleDryRun,
        ),
        PanelView::Grace => grace(settings, lang),
        PanelView::Global => simple_toggle(
            lang,
            t!(
                lang,
                "global_title",
                count = app.cfg.global_reputation_min_bans
            ),
            settings.global_blocklist,
            CallbackAction::ToggleGlobal,
        ),
        PanelView::AutoDelete => simple_toggle(
            lang,
            t!(
                lang,
                "autodelete_title",
                ttl = settings.bot_message_ttl_secs
            ),
            settings.delete_bot_messages,
            CallbackAction::ToggleAutoDelete,
        ),
    }
}

fn main(app: &Arc<App>, settings: &GroupSettings, lang: Lang) -> Screen {
    let text = format!(
        "{}\n{}",
        t!(
            lang,
            "panel_title",
            bot = escape_html(&app.cfg.bot_name),
            chat = escape_html(settings.title.as_deref().unwrap_or("—")),
        ),
        t!(
            lang,
            "panel_state",
            threshold = settings.threshold,
            policy = t!(lang, settings.policy.label_key()),
            action = t!(lang, settings.action.label_key()),
            dryrun = on_off(lang, settings.dry_run),
            grace = grace_label(lang, settings.grace_messages),
            lang = settings.lang.native_name(),
        ),
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            button(
                t!(lang, "panel_btn_threshold"),
                CallbackAction::Panel(PanelView::Threshold),
            ),
            button(
                t!(lang, "panel_btn_policy"),
                CallbackAction::Panel(PanelView::Policy),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_action"),
                CallbackAction::Panel(PanelView::Action),
            ),
            button(
                t!(lang, "panel_btn_dryrun"),
                CallbackAction::Panel(PanelView::DryRun),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_grace"),
                CallbackAction::Panel(PanelView::Grace),
            ),
            button(
                t!(lang, "panel_btn_global"),
                CallbackAction::Panel(PanelView::Global),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_notify"),
                CallbackAction::Panel(PanelView::Notify),
            ),
            button(
                t!(lang, "panel_btn_autodelete"),
                CallbackAction::Panel(PanelView::AutoDelete),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_stats"),
                CallbackAction::Panel(PanelView::Stats),
            ),
            button(
                t!(lang, "panel_btn_lang"),
                CallbackAction::Panel(PanelView::Lang),
            ),
        ],
        vec![
            button(t!(lang, "panel_btn_reset"), CallbackAction::Reset),
            button(t!(lang, "btn_close"), CallbackAction::Close),
        ],
    ]);

    Screen { text, keyboard }
}

fn threshold(settings: &GroupSettings, lang: Lang) -> Screen {
    // Deltas rather than absolute values keep the keyboard short while still
    // reaching any value; the presets cover the range that actually works.
    let steps = vec![
        button("−10".to_owned(), CallbackAction::AdjustThreshold(-10)),
        button("−5".to_owned(), CallbackAction::AdjustThreshold(-5)),
        button(
            format!("{}%", settings.threshold),
            CallbackAction::Panel(PanelView::Threshold),
        ),
        button("+5".to_owned(), CallbackAction::AdjustThreshold(5)),
        button("+10".to_owned(), CallbackAction::AdjustThreshold(10)),
    ];

    Screen {
        text: t!(lang, "threshold_title", value = settings.threshold),
        keyboard: InlineKeyboardMarkup::new(vec![steps, back_row(lang)]),
    }
}

fn policy(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(
        lang,
        "policy_title",
        value = t!(lang, settings.policy.label_key())
    );
    let mut rows = Vec::new();

    for option in Policy::ALL {
        text.push_str(&format!(
            "\n\n<b>{}</b>\n{}",
            t!(lang, option.label_key()),
            t!(lang, option.desc_key())
        ));
        rows.push(vec![button(
            checkbox(option == settings.policy, t!(lang, option.label_key())),
            CallbackAction::SetPolicy(option),
        )]);
    }

    // The custom policy is meaningless without its filter list, so surface the
    // editor right here instead of hiding it a level deeper.
    if settings.policy == Policy::Custom {
        rows.push(vec![button(
            t!(lang, "custom_title_short"),
            CallbackAction::Panel(PanelView::Custom),
        )]);
    }

    rows.push(back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn custom(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(lang, "custom_title");
    if settings.custom_filters.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "custom_none"));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = ALL_FILTERS
        .iter()
        .map(|id| {
            let selected = settings.custom_filters.iter().any(|f| f == id);
            vec![button(
                checkbox(selected, t!(lang, &format!("reason_{id}"))),
                CallbackAction::ToggleCustom((*id).to_owned()),
            )]
        })
        .collect();

    rows.push(vec![button(
        t!(lang, "btn_back"),
        CallbackAction::Panel(PanelView::Policy),
    )]);

    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

/// Which of the verifier's classes this group treats as explicit.
///
/// The list comes from the detector, not from a constant here, so pointing
/// `VERIFIER_MODEL_ID` at a different model changes what admins can pick with
/// no code change.
fn categories(app: &Arc<App>, settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(lang, "categories_title");

    if app.verifier_categories.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "categories_unavailable"));
        return Screen {
            text,
            keyboard: InlineKeyboardMarkup::new(vec![back_row(lang)]),
        };
    }

    if settings.nsfw_categories.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "categories_none"));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = app
        .verifier_categories
        .iter()
        .map(|name| {
            let selected = settings
                .nsfw_categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case(name));
            // Fall back to the raw class name: a swapped-in model may have
            // classes this build has never heard of, and showing `⟦…⟧` would be
            // worse than showing what the model actually calls it.
            let label = match crate::i18n::lookup_opt(lang, &format!("category_{name}")) {
                Some(translated) => translated.to_owned(),
                None => name.clone(),
            };
            vec![button(
                checkbox(selected, label),
                CallbackAction::ToggleCategory(name.clone()),
            )]
        })
        .collect();

    rows.push(back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn action(settings: &GroupSettings, lang: Lang) -> Screen {
    let rows: Vec<Vec<InlineKeyboardButton>> = Action::ALL
        .iter()
        .map(|option| {
            vec![button(
                checkbox(*option == settings.action, t!(lang, option.label_key())),
                CallbackAction::SetAction(*option),
            )]
        })
        .chain(std::iter::once(back_row(lang)))
        .collect();

    Screen {
        text: t!(
            lang,
            "action_title",
            value = t!(lang, settings.action.label_key())
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn language(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Lang::ALL
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|option| {
                    button(
                        checkbox(
                            *option == settings.lang,
                            format!("{} {}", option.flag(), option.native_name()),
                        ),
                        CallbackAction::SetGroupLang(*option),
                    )
                })
                .collect()
        })
        .collect();
    rows.push(back_row(lang));

    Screen {
        text: t!(lang, "lang_title", value = settings.lang.native_name()),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

async fn notify(app: &Arc<App>, settings: &GroupSettings, admin_id: i64, lang: Lang) -> Screen {
    let enabled = db::groups::get_dm_notify(&app.db, settings.chat_id, admin_id)
        .await
        .unwrap_or(false);

    let mut text = t!(lang, "notify_title");

    // A DM preference is useless until the admin has opened a private chat, so
    // say that up front rather than letting the notifications silently fail.
    let reachable = matches!(
        db::users::get(&app.db, admin_id).await,
        Ok(Some(user)) if user.started_bot && !user.is_blocked
    );
    if !reachable {
        text.push_str("\n\n");
        text.push_str(&t!(
            lang,
            "notify_needs_start",
            link = format!("https://t.me/{}", app.cfg.bot_username)
        ));
    }

    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![
            vec![button(
                checkbox(
                    enabled,
                    t!(lang, if enabled { "notify_on" } else { "notify_off" }),
                ),
                CallbackAction::ToggleNotify,
            )],
            back_row(lang),
        ]),
    }
}

async fn stats(app: &Arc<App>, settings: &GroupSettings, lang: Lang) -> Screen {
    let bundle = db::stats::for_group(&app.db, settings.chat_id)
        .await
        .unwrap_or_default();

    let mut text = t!(
        lang,
        "stats_title",
        chat = escape_html(settings.title.as_deref().unwrap_or("—"))
    );

    if bundle.is_empty() {
        text.push_str("\n\n");
        text.push_str(&t!(lang, "stats_none"));
    } else {
        for (key, period) in [
            ("stats_period_day", bundle.day),
            ("stats_period_week", bundle.week),
            ("stats_period_month", bundle.month),
            ("stats_period_all", bundle.all),
        ] {
            text.push('\n');
            text.push_str(&t!(
                lang,
                "stats_row",
                period = t!(lang, key),
                detected = period.detected,
                deleted = period.deleted,
                banned = period.banned,
            ));
        }
    }

    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![back_row(lang)]),
    }
}

fn grace(settings: &GroupSettings, lang: Lang) -> Screen {
    let options = [0, 3, 5, 10, 25];
    let row: Vec<InlineKeyboardButton> = options
        .iter()
        .map(|n| {
            button(
                checkbox(
                    *n == settings.grace_messages,
                    if *n == 0 {
                        "∞".to_owned()
                    } else {
                        n.to_string()
                    },
                ),
                CallbackAction::SetGrace(*n),
            )
        })
        .collect();

    Screen {
        text: t!(
            lang,
            "grace_title",
            value = grace_label(lang, settings.grace_messages)
        ),
        keyboard: InlineKeyboardMarkup::new(vec![row, back_row(lang)]),
    }
}

fn simple_toggle(lang: Lang, text: String, enabled: bool, action: CallbackAction) -> Screen {
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![
            vec![button(checkbox(enabled, on_off(lang, enabled)), action)],
            back_row(lang),
        ]),
    }
}
