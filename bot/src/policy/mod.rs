//! Turns a [`ScanReport`] into a decision.
//!
//! Kept deliberately separate from the filters: filters answer *what is true*
//! about an account, the policy answers *what this group considers actionable*.
//! Two groups can share every signal and still disagree about bans.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::filters::{
    F_BIO_KEYWORDS, F_BIO_LINK, F_MESSAGE_MEDIA, F_NAME_PATTERN, F_PROFILE_NSFW, F_PROFILE_OCR,
    F_REPUTATION, ScanReport,
};

/// What the bot does to a matched account.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Ban,
    Delete,
    Mute,
    /// Detect and report, but change nothing.
    Report,
}

impl Action {
    pub const ALL: [Action; 4] = [Action::Ban, Action::Delete, Action::Mute, Action::Report];

    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Ban => "ban",
            Action::Delete => "delete",
            Action::Mute => "mute",
            Action::Report => "report",
        }
    }

    /// Translation key for the human-readable name.
    pub const fn label_key(self) -> &'static str {
        match self {
            Action::Ban => "action_ban",
            Action::Delete => "action_delete",
            Action::Mute => "action_mute",
            Action::Report => "action_report",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ban" => Ok(Action::Ban),
            "delete" => Ok(Action::Delete),
            "mute" => Ok(Action::Mute),
            "report" => Ok(Action::Report),
            other => Err(format!(
                "unknown action {other:?} (supported: ban, delete, mute, report)"
            )),
        }
    }
}

/// Which combination of signals this group treats as actionable.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// An NSFW image is enough, from the profile or the message.
    NsfwOnly,
    /// NSFW *and* advertising. The default: fewest false positives, because a
    /// person with a racy avatar and no channel to sell is not a spammer.
    #[default]
    NsfwAndContact,
    /// NSFW, or explicit vocabulary in the profile text.
    NsfwOrKeywords,
    /// Any two independent signals agreeing.
    Strict,
    /// Exactly the filters the admins picked; all of them must fire.
    Custom,
}

impl Policy {
    pub const ALL: [Policy; 5] = [
        Policy::NsfwOnly,
        Policy::NsfwAndContact,
        Policy::NsfwOrKeywords,
        Policy::Strict,
        Policy::Custom,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Policy::NsfwOnly => "nsfw_only",
            Policy::NsfwAndContact => "nsfw_and_contact",
            Policy::NsfwOrKeywords => "nsfw_or_keywords",
            Policy::Strict => "strict",
            Policy::Custom => "custom",
        }
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Policy::NsfwOnly => "policy_nsfw_only",
            Policy::NsfwAndContact => "policy_nsfw_and_contact",
            Policy::NsfwOrKeywords => "policy_nsfw_or_keywords",
            Policy::Strict => "policy_strict",
            Policy::Custom => "policy_custom",
        }
    }

    pub const fn desc_key(self) -> &'static str {
        match self {
            Policy::NsfwOnly => "policy_nsfw_only_desc",
            Policy::NsfwAndContact => "policy_nsfw_and_contact_desc",
            Policy::NsfwOrKeywords => "policy_nsfw_or_keywords_desc",
            Policy::Strict => "policy_strict_desc",
            Policy::Custom => "policy_custom_desc",
        }
    }

    /// Filters this policy actually consults. The scanner unions their
    /// [`crate::filters::Needs`] so an unused signal costs no network calls —
    /// a `nsfw_only` group never pays for OCR.
    pub fn relevant_filters<'a>(self, custom: &'a [String]) -> Vec<&'a str> {
        // The shared blocklist is an independent override, so it is relevant
        // under every policy.
        let mut ids: Vec<&'a str> = vec![F_REPUTATION];

        ids.extend(match self {
            Policy::NsfwOnly => vec![F_PROFILE_NSFW, F_MESSAGE_MEDIA],
            Policy::NsfwAndContact => {
                vec![F_PROFILE_NSFW, F_MESSAGE_MEDIA, F_BIO_LINK, F_PROFILE_OCR]
            }
            Policy::NsfwOrKeywords => {
                vec![
                    F_PROFILE_NSFW,
                    F_MESSAGE_MEDIA,
                    F_BIO_KEYWORDS,
                    F_NAME_PATTERN,
                ]
            }
            Policy::Strict => vec![
                F_PROFILE_NSFW,
                F_MESSAGE_MEDIA,
                F_BIO_LINK,
                F_BIO_KEYWORDS,
                F_NAME_PATTERN,
                F_PROFILE_OCR,
            ],
            Policy::Custom => return custom.iter().map(String::as_str).chain(ids).collect(),
        });

        ids
    }
}

impl fmt::Display for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Policy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nsfw_only" => Ok(Policy::NsfwOnly),
            "nsfw_and_contact" => Ok(Policy::NsfwAndContact),
            "nsfw_or_keywords" => Ok(Policy::NsfwOrKeywords),
            "strict" => Ok(Policy::Strict),
            "custom" => Ok(Policy::Custom),
            other => Err(format!("unknown filter policy {other:?}")),
        }
    }
}

/// The decision for one comment.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub matched: bool,
    /// What to actually do — [`Action::Report`] whenever the group is in
    /// dry-run, regardless of the configured action.
    pub action: Action,
    /// Headline NSFW probability, for the report text.
    pub score: f32,
    /// Ids of the filters that fired, sorted.
    pub reasons: Vec<&'static str>,
}

impl Verdict {
    pub fn clean() -> Self {
        Self {
            matched: false,
            action: Action::Report,
            score: 0.0,
            reasons: Vec::new(),
        }
    }
}

/// Does an NSFW *image* signal exist, from either source?
fn nsfw_image(report: &ScanReport) -> bool {
    report.triggered(F_PROFILE_NSFW) || report.triggered(F_MESSAGE_MEDIA)
}

/// Does the account advertise a way to reach it?
fn advertises_contact(report: &ScanReport) -> bool {
    report.triggered(F_BIO_LINK) || report.triggered(F_PROFILE_OCR)
}

/// Apply a group's policy to a completed scan.
pub fn evaluate(
    report: &ScanReport,
    policy: Policy,
    action: Action,
    custom: &[String],
    dry_run: bool,
) -> Verdict {
    // The shared blocklist short-circuits every policy: enough independent
    // groups have already convicted this account.
    let matched = report.triggered(F_REPUTATION)
        || match policy {
            Policy::NsfwOnly => nsfw_image(report),
            // A policy named "NSFW + contact" must never fire without an NSFW
            // signal. It used to also accept `no_photo_link` on its own, which
            // banned a real user at a reported 0% NSFW score.
            Policy::NsfwAndContact => nsfw_image(report) && advertises_contact(report),
            Policy::NsfwOrKeywords => {
                nsfw_image(report)
                    || report.triggered(F_BIO_KEYWORDS)
                    || report.triggered(F_NAME_PATTERN)
            }
            // Two independent signals. Profile-NSFW and message-NSFW are not
            // independent enough on their own, so image signals count once.
            //
            // `no_photo_link` is excluded: it requires a bio link, so counting
            // it alongside `bio_link` would reach two from a single fact.
            Policy::Strict => {
                let image = u8::from(nsfw_image(report));
                let text = u8::from(
                    report.triggered(F_BIO_LINK)
                        || report.triggered(F_BIO_KEYWORDS)
                        || report.triggered(F_NAME_PATTERN),
                );
                let avatar_text = u8::from(report.triggered(F_PROFILE_OCR));
                image + text + avatar_text >= 2
            }
            // An empty custom list would otherwise match everything, so it is
            // treated as "never match" — the UI warns about this too.
            Policy::Custom => !custom.is_empty() && custom.iter().all(|id| report.triggered(id)),
        };

    if !matched {
        return Verdict::clean();
    }

    Verdict {
        matched: true,
        action: if dry_run { Action::Report } else { action },
        score: report.headline_score(),
        reasons: report.triggered_ids(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Still exercised here even though no preset consults it any more — these
    // tests exist precisely to keep it out of the presets.
    use crate::filters::{F_NO_PHOTO_LINK, FilterOutcome};

    /// Build a report where the listed filters fired.
    fn report(triggered: &[&'static str]) -> ScanReport {
        let mut r = ScanReport::default();
        for id in triggered {
            r.insert(id, FilterOutcome::triggered(0.9, None));
        }
        r
    }

    fn matched(triggered: &[&'static str], policy: Policy) -> bool {
        evaluate(&report(triggered), policy, Action::Ban, &[], false).matched
    }

    #[test]
    fn nsfw_only_acts_on_the_image_alone() {
        assert!(matched(&[F_PROFILE_NSFW], Policy::NsfwOnly));
        assert!(matched(&[F_MESSAGE_MEDIA], Policy::NsfwOnly));
        assert!(!matched(&[F_BIO_LINK], Policy::NsfwOnly));
    }

    #[test]
    fn nsfw_and_contact_requires_both_halves() {
        assert!(!matched(&[F_PROFILE_NSFW], Policy::NsfwAndContact));
        assert!(!matched(&[F_BIO_LINK], Policy::NsfwAndContact));
        assert!(matched(
            &[F_PROFILE_NSFW, F_BIO_LINK],
            Policy::NsfwAndContact
        ));
        // Contact info written on the avatar counts as advertising.
        assert!(matched(
            &[F_PROFILE_NSFW, F_PROFILE_OCR],
            Policy::NsfwAndContact
        ));
    }

    /// Regression: this exact combination banned a real group member whose
    /// report read "NSFW probability: 0%". No preset may convict on a hidden
    /// avatar plus a bio link — that is not evidence of NSFW content.
    #[test]
    fn a_hidden_avatar_with_a_link_never_matches_a_preset() {
        for policy in [
            Policy::NsfwOnly,
            Policy::NsfwAndContact,
            Policy::NsfwOrKeywords,
            Policy::Strict,
        ] {
            assert!(
                !matched(&[F_NO_PHOTO_LINK, F_BIO_LINK], policy),
                "{policy} convicted with no NSFW signal at all"
            );
        }
    }

    #[test]
    fn no_preset_ever_fires_without_an_image_or_keyword_signal() {
        // Every weak, purely-structural signal at once still is not enough.
        let weak = &[F_BIO_LINK, F_NO_PHOTO_LINK];
        for policy in [Policy::NsfwOnly, Policy::NsfwAndContact, Policy::Strict] {
            assert!(!matched(weak, policy), "{policy} is too eager");
        }
    }

    #[test]
    fn no_photo_link_is_still_usable_in_a_custom_policy() {
        // It remains a real signal when an admin deliberately pairs it.
        let custom = vec![F_NO_PHOTO_LINK.to_owned(), F_BIO_KEYWORDS.to_owned()];

        assert!(
            !evaluate(
                &report(&[F_NO_PHOTO_LINK]),
                Policy::Custom,
                Action::Ban,
                &custom,
                false
            )
            .matched
        );
        assert!(
            evaluate(
                &report(&[F_NO_PHOTO_LINK, F_BIO_KEYWORDS]),
                Policy::Custom,
                Action::Ban,
                &custom,
                false
            )
            .matched
        );
    }

    #[test]
    fn keywords_alone_suffice_only_under_that_policy() {
        assert!(matched(&[F_BIO_KEYWORDS], Policy::NsfwOrKeywords));
        assert!(!matched(&[F_BIO_KEYWORDS], Policy::NsfwAndContact));
    }

    #[test]
    fn strict_needs_two_independent_signals() {
        assert!(!matched(&[F_PROFILE_NSFW], Policy::Strict));
        // Both image sources are one signal, not two.
        assert!(!matched(&[F_PROFILE_NSFW, F_MESSAGE_MEDIA], Policy::Strict));
        assert!(matched(&[F_PROFILE_NSFW, F_BIO_LINK], Policy::Strict));
        assert!(matched(&[F_BIO_KEYWORDS, F_PROFILE_OCR], Policy::Strict));
    }

    #[test]
    fn custom_requires_every_selected_filter() {
        let custom = vec![F_PROFILE_NSFW.to_owned(), F_BIO_LINK.to_owned()];

        let partial = evaluate(
            &report(&[F_PROFILE_NSFW]),
            Policy::Custom,
            Action::Ban,
            &custom,
            false,
        );
        assert!(!partial.matched);

        let full = evaluate(
            &report(&[F_PROFILE_NSFW, F_BIO_LINK]),
            Policy::Custom,
            Action::Ban,
            &custom,
            false,
        );
        assert!(full.matched);
    }

    #[test]
    fn empty_custom_selection_never_matches() {
        let everything = report(&[F_PROFILE_NSFW, F_BIO_LINK, F_BIO_KEYWORDS]);
        assert!(!evaluate(&everything, Policy::Custom, Action::Ban, &[], false).matched);
    }

    #[test]
    fn shared_blocklist_overrides_any_policy() {
        // Would not match nsfw_and_contact on its own signals.
        assert!(matched(&[F_REPUTATION], Policy::NsfwAndContact));
        assert!(matched(&[F_REPUTATION], Policy::Custom));
    }

    #[test]
    fn dry_run_downgrades_the_action_but_keeps_the_match() {
        let v = evaluate(
            &report(&[F_PROFILE_NSFW]),
            Policy::NsfwOnly,
            Action::Ban,
            &[],
            true,
        );
        assert!(v.matched);
        assert_eq!(v.action, Action::Report);
    }

    #[test]
    fn relevant_filters_stay_narrow_for_cheap_policies() {
        let ids = Policy::NsfwOnly.relevant_filters(&[]);
        assert!(
            !ids.contains(&F_PROFILE_OCR),
            "nsfw_only must not pay for OCR"
        );

        let ids = Policy::NsfwAndContact.relevant_filters(&[]);
        assert!(ids.contains(&F_PROFILE_OCR) && ids.contains(&F_BIO_LINK));
    }

    #[test]
    fn custom_policy_reports_its_own_filters_plus_reputation() {
        let custom = vec![F_BIO_KEYWORDS.to_owned()];
        let ids = Policy::Custom.relevant_filters(&custom);
        assert!(ids.contains(&F_BIO_KEYWORDS));
        assert!(ids.contains(&F_REPUTATION));
    }

    #[test]
    fn round_trips_through_strings() {
        for policy in Policy::ALL {
            assert_eq!(policy.as_str().parse::<Policy>().unwrap(), policy);
        }
        for action in Action::ALL {
            assert_eq!(action.as_str().parse::<Action>().unwrap(), action);
        }
    }
}
