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
    db::{
        self,
        models::{GroupSettings, MediaKind, TextTopic},
    },
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

/// The media scan in one line: off, or on with the number that governs it.
///
/// The threshold is on the summary because it is the setting most likely to be
/// confused with the profile one directly above it.
fn media_label(lang: Lang, settings: &GroupSettings) -> String {
    if !settings.media_scan {
        return t!(lang, "state_off");
    }
    t!(lang, "media_state_on", threshold = settings.media_threshold)
}

/// The text scan in one line: off, or on with the number that governs it.
fn text_label(lang: Lang, settings: &GroupSettings) -> String {
    if !settings.text_scan {
        return t!(lang, "state_off");
    }
    if settings.text_topics.is_empty() {
        return t!(lang, "text_state_empty");
    }
    t!(
        lang,
        "text_state_on",
        count = settings.text_topics.len(),
        threshold = settings.text_threshold
    )
}

fn ad_label(lang: Lang, settings: &GroupSettings) -> String {
    if !settings.ad_scan {
        return t!(lang, "state_off");
    }
    t!(lang, "ad_state_on", threshold = settings.ad_threshold)
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
        PanelView::BotGuard => simple_toggle(
            lang,
            t!(lang, "botguard_title"),
            settings.ban_foreign_bots,
            CallbackAction::ToggleBotGuard,
        ),
        PanelView::Text => text(app, settings, lang),
        PanelView::TextTopics => text_topics(settings, lang),
        PanelView::TextThreshold => text_threshold(settings, lang),
        PanelView::TextAction => text_action(settings, lang),
        PanelView::Ad => ad(app, settings, lang),
        PanelView::AdThreshold => ad_threshold(settings, lang),
        PanelView::AdAction => ad_action(settings, lang),
        PanelView::Reaction => simple_toggle(
            lang,
            t!(lang, "reaction_title"),
            settings.reaction_scan,
            CallbackAction::ToggleReactionScan,
        ),
        PanelView::Media => media(settings, lang),
        PanelView::MediaKinds => media_kinds(settings, lang),
        PanelView::MediaThreshold => media_threshold(settings, lang),
        PanelView::MediaAction => media_action(settings, lang),
        PanelView::MediaFrames => media_frames(settings, lang),
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
            media = media_label(lang, settings),
            text = text_label(lang, settings),
            ads = ad_label(lang, settings),
            reactions = on_off(lang, settings.reaction_scan),
            lang = settings.lang.native_name(),
        ),
    );

    let keyboard = main_keyboard(lang);

    Screen { text, keyboard }
}

/// The root keyboard, separated so a test can walk it without an [`App`].
fn main_keyboard(lang: Lang) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
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
                t!(lang, "panel_btn_categories"),
                CallbackAction::Panel(PanelView::Categories),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_dryrun"),
                CallbackAction::Panel(PanelView::DryRun),
            ),
            button(
                t!(lang, "panel_btn_grace"),
                CallbackAction::Panel(PanelView::Grace),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_global"),
                CallbackAction::Panel(PanelView::Global),
            ),
            button(
                t!(lang, "panel_btn_notify"),
                CallbackAction::Panel(PanelView::Notify),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_autodelete"),
                CallbackAction::Panel(PanelView::AutoDelete),
            ),
            button(
                t!(lang, "panel_btn_stats"),
                CallbackAction::Panel(PanelView::Stats),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_media"),
                CallbackAction::Panel(PanelView::Media),
            ),
            button(
                t!(lang, "panel_btn_lang"),
                CallbackAction::Panel(PanelView::Lang),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_text"),
                CallbackAction::Panel(PanelView::Text),
            ),
            button(
                t!(lang, "panel_btn_ad"),
                CallbackAction::Panel(PanelView::Ad),
            ),
        ],
        vec![
            button(
                t!(lang, "panel_btn_reaction"),
                CallbackAction::Panel(PanelView::Reaction),
            ),
            button(
                t!(lang, "panel_btn_botguard"),
                CallbackAction::Panel(PanelView::BotGuard),
            ),
        ],
        vec![
            button(t!(lang, "panel_btn_reset"), CallbackAction::Reset),
            button(t!(lang, "btn_close"), CallbackAction::Close),
        ],
    ])
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

// --------------------------------------------------------- message text --

/// Whether Jev is configured at all.
///
/// Every screen below needs it, and a panel that lets an admin tick topics and
/// set a threshold that can never take effect is worse than one that says so.
fn jev_missing(app: &Arc<App>, lang: Lang) -> Option<String> {
    (!app.jev.enabled()).then(|| format!("\n\n⚠️ {}", t!(lang, "jev_unavailable")))
}

/// The hub for judging what people write.
fn text(app: &Arc<App>, settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(
        lang,
        "text_title",
        state = on_off(lang, settings.text_scan),
        threshold = settings.text_threshold,
        action = t!(lang, settings.text_action.label_key()),
        topics = topics_summary(settings, lang),
    );

    if let Some(warning) = jev_missing(app, lang) {
        text.push_str(&warning);
    }

    // A scan that is on and watching nothing is a switch that does nothing.
    if settings.text_scan && settings.text_topics.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "text_topics_none"));
    }

    let mut rows = vec![vec![button(
        checkbox(settings.text_scan, t!(lang, "text_btn_enable")),
        CallbackAction::ToggleTextScan,
    )]];

    if settings.text_scan {
        rows.push(vec![button(
            t!(lang, "text_btn_topics"),
            CallbackAction::Panel(PanelView::TextTopics),
        )]);
        rows.push(vec![
            button(
                t!(lang, "text_btn_threshold"),
                CallbackAction::Panel(PanelView::TextThreshold),
            ),
            button(
                t!(lang, "text_btn_action"),
                CallbackAction::Panel(PanelView::TextAction),
            ),
        ]);
    }

    rows.push(back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

/// The chosen subjects as a short list, or "none".
fn topics_summary(settings: &GroupSettings, lang: Lang) -> String {
    if settings.text_topics.is_empty() {
        return t!(lang, "text_topics_empty");
    }

    // Iterated over ALL rather than over the stored list so the order is fixed
    // and does not shuffle as an admin toggles things.
    TextTopic::ALL
        .iter()
        .filter(|topic| settings.text_topics.contains(topic))
        .map(|topic| t!(lang, topic.label_key()))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn text_topics(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(lang, "text_topics_title");
    if settings.text_topics.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "text_topics_none"));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = TextTopic::ALL
        .iter()
        .map(|topic| {
            vec![button(
                checkbox(
                    settings.text_topics.contains(topic),
                    t!(lang, topic.label_key()),
                ),
                CallbackAction::ToggleTextTopic(*topic),
            )]
        })
        .collect();

    rows.push(text_back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn text_threshold(settings: &GroupSettings, lang: Lang) -> Screen {
    let steps = vec![
        button("−10".to_owned(), CallbackAction::AdjustTextThreshold(-10)),
        button("−5".to_owned(), CallbackAction::AdjustTextThreshold(-5)),
        button(
            format!("{}%", settings.text_threshold),
            CallbackAction::Panel(PanelView::TextThreshold),
        ),
        button("+5".to_owned(), CallbackAction::AdjustTextThreshold(5)),
        button("+10".to_owned(), CallbackAction::AdjustTextThreshold(10)),
    ];

    Screen {
        text: t!(
            lang,
            "text_threshold_title",
            value = settings.text_threshold
        ),
        keyboard: InlineKeyboardMarkup::new(vec![steps, text_back_row(lang)]),
    }
}

fn text_action(settings: &GroupSettings, lang: Lang) -> Screen {
    let rows: Vec<Vec<InlineKeyboardButton>> = Action::ALL
        .iter()
        .map(|option| {
            vec![button(
                checkbox(
                    *option == settings.text_action,
                    t!(lang, option.label_key()),
                ),
                CallbackAction::SetTextAction(*option),
            )]
        })
        .chain(std::iter::once(text_back_row(lang)))
        .collect();

    Screen {
        text: t!(
            lang,
            "text_action_title",
            value = t!(lang, settings.text_action.label_key())
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn text_back_row(lang: Lang) -> Vec<InlineKeyboardButton> {
    vec![
        button(t!(lang, "btn_back"), CallbackAction::Panel(PanelView::Text)),
        button(t!(lang, "btn_close"), CallbackAction::Close),
    ]
}

// ------------------------------------------------------- advertising guard --

fn ad(app: &Arc<App>, settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(
        lang,
        "ad_title",
        state = on_off(lang, settings.ad_scan),
        threshold = settings.ad_threshold,
        action = t!(lang, settings.ad_action.label_key()),
    );

    if let Some(warning) = jev_missing(app, lang) {
        text.push_str(&warning);
    }

    let mut rows = vec![vec![button(
        checkbox(settings.ad_scan, t!(lang, "ad_btn_enable")),
        CallbackAction::ToggleAdScan,
    )]];

    if settings.ad_scan {
        rows.push(vec![
            button(
                t!(lang, "ad_btn_threshold"),
                CallbackAction::Panel(PanelView::AdThreshold),
            ),
            button(
                t!(lang, "ad_btn_action"),
                CallbackAction::Panel(PanelView::AdAction),
            ),
        ]);
    }

    rows.push(back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn ad_threshold(settings: &GroupSettings, lang: Lang) -> Screen {
    let steps = vec![
        button("−10".to_owned(), CallbackAction::AdjustAdThreshold(-10)),
        button("−5".to_owned(), CallbackAction::AdjustAdThreshold(-5)),
        button(
            format!("{}%", settings.ad_threshold),
            CallbackAction::Panel(PanelView::AdThreshold),
        ),
        button("+5".to_owned(), CallbackAction::AdjustAdThreshold(5)),
        button("+10".to_owned(), CallbackAction::AdjustAdThreshold(10)),
    ];

    Screen {
        text: t!(lang, "ad_threshold_title", value = settings.ad_threshold),
        keyboard: InlineKeyboardMarkup::new(vec![steps, ad_back_row(lang)]),
    }
}

fn ad_action(settings: &GroupSettings, lang: Lang) -> Screen {
    let rows: Vec<Vec<InlineKeyboardButton>> = Action::ALL
        .iter()
        .map(|option| {
            vec![button(
                checkbox(*option == settings.ad_action, t!(lang, option.label_key())),
                CallbackAction::SetAdAction(*option),
            )]
        })
        .chain(std::iter::once(ad_back_row(lang)))
        .collect();

    Screen {
        text: t!(
            lang,
            "ad_action_title",
            value = t!(lang, settings.ad_action.label_key())
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn ad_back_row(lang: Lang) -> Vec<InlineKeyboardButton> {
    vec![
        button(t!(lang, "btn_back"), CallbackAction::Panel(PanelView::Ad)),
        button(t!(lang, "btn_close"), CallbackAction::Close),
    ]
}

// --------------------------------------------------------- group media scan --

/// The hub for scanning what people post, as opposed to who they are.
///
/// Its own screen rather than a row in the main panel because it is a genuinely
/// separate mechanism with its own threshold, action and scope, and folding it
/// into the profile settings is exactly the confusion that would lead an admin
/// to set one number and expect it to govern the other.
fn media(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(
        lang,
        "media_title",
        state = on_off(lang, settings.media_scan),
        threshold = settings.media_threshold,
        action = t!(lang, settings.media_action.label_key()),
        kinds = kinds_summary(settings, lang),
        frames = settings.media_frames,
    );

    // An enabled scan covering nothing is a switch that does nothing, and the
    // panel should say so rather than let an admin believe they are protected.
    if settings.media_scan && settings.media_kinds.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "media_kinds_none"));
    }

    let mut rows = vec![vec![button(
        checkbox(settings.media_scan, t!(lang, "media_btn_enable")),
        CallbackAction::ToggleMediaScan,
    )]];

    // The rest only matter once it is on; showing them beforehand invites
    // tuning a threshold that is not in force.
    if settings.media_scan {
        rows.push(vec![
            button(
                t!(lang, "media_btn_threshold"),
                CallbackAction::Panel(PanelView::MediaThreshold),
            ),
            button(
                t!(lang, "media_btn_action"),
                CallbackAction::Panel(PanelView::MediaAction),
            ),
        ]);
        rows.push(vec![
            button(
                t!(lang, "media_btn_kinds"),
                CallbackAction::Panel(PanelView::MediaKinds),
            ),
            button(
                t!(lang, "media_btn_frames"),
                CallbackAction::Panel(PanelView::MediaFrames),
            ),
        ]);
    }

    rows.push(back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

/// The enabled kinds as a short list, or "none".
fn kinds_summary(settings: &GroupSettings, lang: Lang) -> String {
    if settings.media_kinds.is_empty() {
        return t!(lang, "media_kinds_empty");
    }

    // Iterated over ALL rather than over the stored list so the order is fixed
    // and does not shuffle as an admin toggles things.
    MediaKind::ALL
        .iter()
        .filter(|kind| settings.media_kinds.contains(kind))
        .map(|kind| t!(lang, kind.label_key()))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn media_threshold(settings: &GroupSettings, lang: Lang) -> Screen {
    let steps = vec![
        button("−10".to_owned(), CallbackAction::AdjustMediaThreshold(-10)),
        button("−5".to_owned(), CallbackAction::AdjustMediaThreshold(-5)),
        button(
            format!("{}%", settings.media_threshold),
            CallbackAction::Panel(PanelView::MediaThreshold),
        ),
        button("+5".to_owned(), CallbackAction::AdjustMediaThreshold(5)),
        button("+10".to_owned(), CallbackAction::AdjustMediaThreshold(10)),
    ];

    Screen {
        text: t!(
            lang,
            "media_threshold_title",
            value = settings.media_threshold,
            profile = settings.threshold,
        ),
        keyboard: InlineKeyboardMarkup::new(vec![steps, media_back_row(lang)]),
    }
}

fn media_action(settings: &GroupSettings, lang: Lang) -> Screen {
    let rows: Vec<Vec<InlineKeyboardButton>> = Action::ALL
        .iter()
        .map(|option| {
            vec![button(
                checkbox(
                    *option == settings.media_action,
                    t!(lang, option.label_key()),
                ),
                CallbackAction::SetMediaAction(*option),
            )]
        })
        .chain(std::iter::once(media_back_row(lang)))
        .collect();

    Screen {
        text: t!(
            lang,
            "media_action_title",
            value = t!(lang, settings.media_action.label_key()),
            profile = t!(lang, settings.action.label_key()),
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn media_kinds(settings: &GroupSettings, lang: Lang) -> Screen {
    let mut text = t!(lang, "media_kinds_title");
    if settings.media_kinds.is_empty() {
        text.push_str("\n\n⚠️ ");
        text.push_str(&t!(lang, "media_kinds_none"));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = MediaKind::ALL
        .iter()
        .map(|kind| {
            vec![button(
                checkbox(
                    settings.media_kinds.contains(kind),
                    t!(lang, kind.label_key()),
                ),
                CallbackAction::ToggleMediaKind(*kind),
            )]
        })
        .collect();

    rows.push(media_back_row(lang));
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn media_frames(settings: &GroupSettings, lang: Lang) -> Screen {
    // 1 is offered deliberately: it is the old thumbnail-only behaviour, and a
    // group on a small server may want exactly that.
    let options: [i16; 4] = [1, 3, 5, 9];
    let row: Vec<InlineKeyboardButton> = options
        .iter()
        .map(|n| {
            button(
                checkbox(*n == settings.media_frames, n.to_string()),
                CallbackAction::SetMediaFrames(*n),
            )
        })
        .collect();

    Screen {
        text: t!(lang, "media_frames_title", value = settings.media_frames),
        keyboard: InlineKeyboardMarkup::new(vec![row, media_back_row(lang)]),
    }
}

/// Back to the media hub rather than the main panel: these are its sub-screens,
/// and landing on the main menu would lose the admin's place.
fn media_back_row(lang: Lang) -> Vec<InlineKeyboardButton> {
    vec![
        button(
            t!(lang, "btn_back"),
            CallbackAction::Panel(PanelView::Media),
        ),
        button(t!(lang, "btn_close"), CallbackAction::Close),
    ]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every settings screen must be reachable from the main panel.
    ///
    /// The categories screen shipped once with its renderer, its callback and
    /// its translations all correct — and no button, because a text edit to the
    /// keyboard silently failed to apply. Nothing caught it: the code compiled,
    /// the tests passed, and the feature was simply invisible.
    ///
    /// This walks the real keyboard rather than a list maintained by hand, so a
    /// new screen that nobody linked to fails here.
    #[test]
    fn every_panel_view_is_reachable_from_the_main_screen() {
        // Screens reached from inside another screen rather than the root.
        const NESTED: &[PanelView] = &[
            PanelView::Main,
            // Opened from the Policy screen, once `custom` is selected.
            PanelView::Custom,
        ];

        let reachable = main_screen_targets();

        for view in PanelView::ALL {
            if NESTED.contains(&view) {
                continue;
            }
            assert!(
                reachable.contains(&view),
                "{view:?} has no button on the main panel — it is unreachable"
            );
        }
    }

    /// Decode the main keyboard back into the views it links to.
    ///
    /// Going through the encoded callback data is deliberate: it exercises the
    /// same round trip Telegram performs, so a button wired to a payload that
    /// does not decode fails here too.
    fn main_screen_targets() -> Vec<PanelView> {
        keyboard_targets(&main_keyboard(Lang::En))
    }

    fn keyboard_targets(keyboard: &InlineKeyboardMarkup) -> Vec<PanelView> {
        keyboard
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|b| match &b.kind {
                teloxide::types::InlineKeyboardButtonKind::CallbackData(data) => {
                    match CallbackAction::decode(data) {
                        Some(CallbackAction::Panel(view)) => Some(view),
                        Some(_) => None,
                        None => panic!("button {:?} carries undecodable callback data", b.text),
                    }
                }
                _ => None,
            })
            .collect()
    }
}
