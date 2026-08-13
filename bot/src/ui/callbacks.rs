//! Callback-data encoding.
//!
//! Telegram caps `callback_data` at 64 bytes, so the format is a terse
//! colon-separated tuple rather than JSON. Everything round-trips through
//! [`CallbackAction`], which is the single place that knows the wire format —
//! no handler parses raw strings.

use std::str::FromStr;

use crate::{
    i18n::Lang,
    policy::{Action, Policy},
};

/// Which screen of the group settings panel to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Threshold,
    Policy,
    Custom,
    Categories,
    Action,
    Lang,
    Notify,
    Stats,
    DryRun,
    Grace,
    Global,
    AutoDelete,
}

impl PanelView {
    const fn code(self) -> &'static str {
        match self {
            PanelView::Main => "m",
            PanelView::Threshold => "th",
            PanelView::Policy => "po",
            PanelView::Custom => "cu",
            PanelView::Categories => "ca",
            PanelView::Action => "ac",
            PanelView::Lang => "la",
            PanelView::Notify => "no",
            PanelView::Stats => "st",
            PanelView::DryRun => "dr",
            PanelView::Grace => "gr",
            PanelView::Global => "gl",
            PanelView::AutoDelete => "ad",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "m" => PanelView::Main,
            "th" => PanelView::Threshold,
            "po" => PanelView::Policy,
            "cu" => PanelView::Custom,
            "ca" => PanelView::Categories,
            "ac" => PanelView::Action,
            "la" => PanelView::Lang,
            "no" => PanelView::Notify,
            "st" => PanelView::Stats,
            "dr" => PanelView::DryRun,
            "gr" => PanelView::Grace,
            "gl" => PanelView::Global,
            "ad" => PanelView::AutoDelete,
            _ => return None,
        })
    }
}

/// Which screen of the private-chat menu to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmView {
    Start,
    Help,
    Test,
    Lang,
}

impl PmView {
    const fn code(self) -> &'static str {
        match self {
            PmView::Start => "s",
            PmView::Help => "h",
            PmView::Test => "t",
            PmView::Lang => "l",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "s" => PmView::Start,
            "h" => PmView::Help,
            "t" => PmView::Test,
            "l" => PmView::Lang,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackAction {
    /// Open a settings screen.
    Panel(PanelView),
    /// Nudge the threshold by a signed delta, clamped by the handler.
    AdjustThreshold(i16),
    SetPolicy(Policy),
    /// Add or remove a filter from the custom policy.
    ToggleCustom(String),
    /// Add or remove one of the verifier's classes from what counts as NSFW.
    ToggleCategory(String),
    SetAction(Action),
    SetGroupLang(Lang),
    ToggleNotify,
    ToggleDryRun,
    ToggleGlobal,
    ToggleAutoDelete,
    SetGrace(i32),
    Reset,
    Close,

    /// An admin says a detection was wrong; the id is `detections.id`.
    FalsePositive(i64),
    Details(i64),

    /// A banned user appeals in the group identified by the id.
    Appeal(i64),
    AppealResolve {
        chat_id: i64,
        user_id: i64,
        approve: bool,
    },

    /// Private-chat menu.
    Pm(PmView),
    SetUserLang(Lang),

    BroadcastConfirm,
    BroadcastCancel,
}

impl CallbackAction {
    pub fn encode(&self) -> String {
        match self {
            CallbackAction::Panel(view) => format!("p:{}", view.code()),
            CallbackAction::AdjustThreshold(delta) => format!("p:thd:{delta}"),
            CallbackAction::SetPolicy(policy) => format!("p:pos:{policy}"),
            CallbackAction::ToggleCustom(filter) => format!("p:cut:{filter}"),
            CallbackAction::ToggleCategory(name) => format!("p:cat:{name}"),
            CallbackAction::SetAction(action) => format!("p:acs:{action}"),
            CallbackAction::SetGroupLang(lang) => format!("p:las:{lang}"),
            CallbackAction::ToggleNotify => "p:not".to_owned(),
            CallbackAction::ToggleDryRun => "p:drt".to_owned(),
            CallbackAction::ToggleGlobal => "p:glt".to_owned(),
            CallbackAction::ToggleAutoDelete => "p:adt".to_owned(),
            CallbackAction::SetGrace(n) => format!("p:grs:{n}"),
            CallbackAction::Reset => "p:rst".to_owned(),
            CallbackAction::Close => "p:x".to_owned(),

            CallbackAction::FalsePositive(id) => format!("d:fp:{id}"),
            CallbackAction::Details(id) => format!("d:dt:{id}"),

            CallbackAction::Appeal(chat_id) => format!("a:new:{chat_id}"),
            CallbackAction::AppealResolve {
                chat_id,
                user_id,
                approve,
            } => {
                format!(
                    "a:{}:{chat_id}:{user_id}",
                    if *approve { "ok" } else { "no" }
                )
            }

            CallbackAction::Pm(view) => format!("u:{}", view.code()),
            CallbackAction::SetUserLang(lang) => format!("u:las:{lang}"),

            CallbackAction::BroadcastConfirm => "b:go".to_owned(),
            CallbackAction::BroadcastCancel => "b:no".to_owned(),
        }
    }

    pub fn decode(data: &str) -> Option<Self> {
        let parts: Vec<&str> = data.split(':').collect();

        Some(match parts.as_slice() {
            ["p", "thd", delta] => CallbackAction::AdjustThreshold(delta.parse().ok()?),
            ["p", "pos", policy] => CallbackAction::SetPolicy(Policy::from_str(policy).ok()?),
            ["p", "cut", filter] => CallbackAction::ToggleCustom((*filter).to_owned()),
            ["p", "cat", name] => CallbackAction::ToggleCategory((*name).to_owned()),
            ["p", "acs", action] => CallbackAction::SetAction(Action::from_str(action).ok()?),
            ["p", "las", lang] => CallbackAction::SetGroupLang(Lang::from_str(lang).ok()?),
            ["p", "not"] => CallbackAction::ToggleNotify,
            ["p", "drt"] => CallbackAction::ToggleDryRun,
            ["p", "glt"] => CallbackAction::ToggleGlobal,
            ["p", "adt"] => CallbackAction::ToggleAutoDelete,
            ["p", "grs", n] => CallbackAction::SetGrace(n.parse().ok()?),
            ["p", "rst"] => CallbackAction::Reset,
            ["p", "x"] => CallbackAction::Close,
            // Must come last among the `p:` arms: the two-part patterns above
            // are more specific than a bare view code.
            ["p", view] => CallbackAction::Panel(PanelView::parse(view)?),

            ["d", "fp", id] => CallbackAction::FalsePositive(id.parse().ok()?),
            ["d", "dt", id] => CallbackAction::Details(id.parse().ok()?),

            ["a", "new", chat] => CallbackAction::Appeal(chat.parse().ok()?),
            ["a", verdict @ ("ok" | "no"), chat, user] => CallbackAction::AppealResolve {
                chat_id: chat.parse().ok()?,
                user_id: user.parse().ok()?,
                approve: *verdict == "ok",
            },

            ["u", "las", lang] => CallbackAction::SetUserLang(Lang::from_str(lang).ok()?),
            ["u", view] => CallbackAction::Pm(PmView::parse(view)?),

            ["b", "go"] => CallbackAction::BroadcastConfirm,
            ["b", "no"] => CallbackAction::BroadcastCancel,

            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::ALL_FILTERS;

    fn round_trip(action: CallbackAction) {
        let encoded = action.encode();
        assert!(
            encoded.len() <= 64,
            "callback data {encoded:?} exceeds Telegram's 64-byte limit"
        );
        assert_eq!(
            CallbackAction::decode(&encoded),
            Some(action),
            "failed to decode {encoded:?}"
        );
    }

    #[test]
    fn every_panel_view_round_trips() {
        for view in [
            PanelView::Main,
            PanelView::Threshold,
            PanelView::Policy,
            PanelView::Custom,
            PanelView::Categories,
            PanelView::Action,
            PanelView::Lang,
            PanelView::Notify,
            PanelView::Stats,
            PanelView::DryRun,
            PanelView::Grace,
            PanelView::Global,
            PanelView::AutoDelete,
        ] {
            round_trip(CallbackAction::Panel(view));
        }
    }

    #[test]
    fn setting_changes_round_trip() {
        round_trip(CallbackAction::AdjustThreshold(5));
        round_trip(CallbackAction::AdjustThreshold(-5));
        round_trip(CallbackAction::SetGrace(0));
        round_trip(CallbackAction::Reset);
        round_trip(CallbackAction::Close);
        round_trip(CallbackAction::ToggleNotify);
        round_trip(CallbackAction::ToggleDryRun);
        round_trip(CallbackAction::ToggleGlobal);
        round_trip(CallbackAction::ToggleAutoDelete);

        for policy in Policy::ALL {
            round_trip(CallbackAction::SetPolicy(policy));
        }
        for action in Action::ALL {
            round_trip(CallbackAction::SetAction(action));
        }
        for lang in Lang::ALL {
            round_trip(CallbackAction::SetGroupLang(lang));
            round_trip(CallbackAction::SetUserLang(lang));
        }
    }

    #[test]
    fn every_filter_id_fits_in_a_toggle() {
        for id in ALL_FILTERS {
            round_trip(CallbackAction::ToggleCustom((*id).to_owned()));
        }
    }

    #[test]
    fn detection_and_appeal_actions_round_trip() {
        round_trip(CallbackAction::FalsePositive(9_223_372_036_854_775_807));
        round_trip(CallbackAction::Details(1));
        round_trip(CallbackAction::Appeal(-1_001_234_567_890));
        round_trip(CallbackAction::AppealResolve {
            chat_id: -1_001_234_567_890,
            user_id: 7_654_321,
            approve: true,
        });
        round_trip(CallbackAction::AppealResolve {
            chat_id: -1_001_234_567_890,
            user_id: 7_654_321,
            approve: false,
        });
    }

    #[test]
    fn pm_views_round_trip() {
        for view in [PmView::Start, PmView::Help, PmView::Test, PmView::Lang] {
            round_trip(CallbackAction::Pm(view));
        }
        round_trip(CallbackAction::BroadcastConfirm);
        round_trip(CallbackAction::BroadcastCancel);
    }

    #[test]
    fn garbage_decodes_to_none() {
        for junk in ["", "x", "p", "p:zz", "d:fp:notanumber", "a:ok:1", "::::"] {
            assert_eq!(
                CallbackAction::decode(junk),
                None,
                "{junk:?} should not decode"
            );
        }
    }
}
