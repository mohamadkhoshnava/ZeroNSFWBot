//! Turns a [`ScanReport`] into a decision.
//!
//! Kept deliberately separate from the filters: filters answer *what is true*
//! about an account, the policy answers *what this group considers actionable*.
//! Two groups can share every signal and still disagree about bans.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::filters::{
    F_BIO_KEYWORDS, F_BIO_LINK, F_BIO_SEMANTIC, F_MESSAGE_MEDIA, F_NAME_PATTERN, F_PROFILE_CHANNEL,
    F_PROFILE_NSFW, F_PROFILE_OCR, F_REPUTATION, ScanReport,
};

/// What the bot does to a matched account.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Ban,
    Delete,
    Mute,
    /// Reply to the message with a public warning, and change nothing else.
    ///
    /// The mildest thing that is still visible to the person who did it, which
    /// is what an advertising rule wants on a first offence: most people who
    /// drop a referral link in a group are members, not spam accounts, and
    /// deleting their message without a word reads as the bot malfunctioning.
    Warn,
    /// Detect and report, but change nothing.
    Report,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::Ban,
        Action::Delete,
        Action::Mute,
        Action::Warn,
        Action::Report,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Ban => "ban",
            Action::Delete => "delete",
            Action::Mute => "mute",
            Action::Warn => "warn",
            Action::Report => "report",
        }
    }

    /// How much this action actually does to someone, for picking between two
    /// findings about the same message. A message that is both an advert and
    /// off-topic is dealt with once, at the stronger of the two settings.
    pub const fn severity(self) -> u8 {
        match self {
            Action::Report => 0,
            Action::Warn => 1,
            Action::Delete => 2,
            Action::Mute => 3,
            Action::Ban => 4,
        }
    }

    /// The harsher of two actions.
    pub fn strongest(self, other: Self) -> Self {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }

    /// Translation key for the human-readable name.
    pub const fn label_key(self) -> &'static str {
        match self {
            Action::Ban => "action_ban",
            Action::Delete => "action_delete",
            Action::Mute => "action_mute",
            Action::Warn => "action_warn",
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
            "warn" => Ok(Action::Warn),
            "report" => Ok(Action::Report),
            other => Err(format!(
                "unknown action {other:?} (supported: ban, delete, mute, warn, report)"
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
                vec![
                    F_PROFILE_NSFW,
                    F_MESSAGE_MEDIA,
                    F_BIO_LINK,
                    F_PROFILE_CHANNEL,
                    F_PROFILE_OCR,
                ]
            }
            Policy::NsfwOrKeywords => {
                vec![
                    F_PROFILE_NSFW,
                    F_MESSAGE_MEDIA,
                    F_BIO_KEYWORDS,
                    F_BIO_SEMANTIC,
                    F_NAME_PATTERN,
                ]
            }
            Policy::Strict => vec![
                F_PROFILE_NSFW,
                F_MESSAGE_MEDIA,
                F_BIO_LINK,
                F_PROFILE_CHANNEL,
                F_BIO_KEYWORDS,
                F_BIO_SEMANTIC,
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
///
/// The three places the same fact can be written: the bio text, the avatar
/// image, and the channel attached to the profile. A spam account only needs
/// one of them, so a policy that reads only the bio misses the ones that leave
/// it empty and pin a channel instead.
fn advertises_contact(report: &ScanReport) -> bool {
    report.triggered(F_BIO_LINK)
        || report.triggered(F_PROFILE_OCR)
        || report.triggered(F_PROFILE_CHANNEL)
}

/// Does the profile *text* read as an adult advertisement?
///
/// One bucket, three ways of reaching it: the hardcoded vocabulary list, the
/// shape of the display name, and Jev's reading of the whole profile. They are
/// not independent — the model fires on exactly the bios the word list was
/// written for, and then on the ones it was not — so `strict` must count them
/// once, or an obfuscated bio (`s3x`, `س‌ک‌س`) that trips both the model and a
/// loosened pattern would reach the two-signal bar on a single fact.
fn advertising_vocabulary(report: &ScanReport) -> bool {
    report.triggered(F_BIO_KEYWORDS)
        || report.triggered(F_NAME_PATTERN)
        || report.triggered(F_BIO_SEMANTIC)
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
            Policy::NsfwOrKeywords => nsfw_image(report) || advertising_vocabulary(report),
            // Two independent signals. Profile-NSFW and message-NSFW are not
            // independent enough on their own, so image signals count once.
            //
            // `no_photo_link` is excluded: it requires a bio link, so counting
            // it alongside `bio_link` would reach two from a single fact.
            Policy::Strict => {
                // Three genuinely independent categories. `bio_link`,
                // `profile_ocr` and `profile_channel` share a bucket because
                // they are the same fact — "this account publishes a way to
                // reach it" — just written in different places. Counting them
                // separately reached two from one fact and would ban, say, a
                // business whose logo carries the same website that is in its
                // bio, or anyone who both links and pins their own channel.
                let image = u8::from(nsfw_image(report));
                let contact = u8::from(advertises_contact(report));
                let vocabulary = u8::from(advertising_vocabulary(report));
                image + contact + vocabulary >= 2
            }
            // An empty custom list would otherwise match everything, so it is
            // treated as "never match" — the UI warns about this too.
            Policy::Custom => !custom.is_empty() && custom.iter().all(|id| report.triggered(id)),
        };

    if !matched {
        return Verdict::clean();
    }

    // Report only the signals this policy actually consulted. Every filter
    // runs, so without this a `nsfw_and_contact` group would see reasons it
    // does not act on listed as the grounds for a ban — which is exactly the
    // information an admin uses to judge whether the bot was right.
    let consulted = policy.relevant_filters(custom);

    Verdict {
        matched: true,
        action: if dry_run { Action::Report } else { action },
        score: report.headline_score(),
        reasons: report
            .triggered_ids()
            .into_iter()
            .filter(|id| consulted.contains(id))
            .collect(),
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

    #[test]
    fn an_attached_channel_is_advertising_like_a_bio_link() {
        // The default preset must act on it, and must still need the NSFW half.
        assert!(matched(
            &[F_PROFILE_NSFW, F_PROFILE_CHANNEL],
            Policy::NsfwAndContact
        ));
        assert!(!matched(&[F_PROFILE_CHANNEL], Policy::NsfwAndContact));
        assert!(matched(
            &[F_PROFILE_NSFW, F_PROFILE_CHANNEL],
            Policy::Strict
        ));
    }

    /// A bio link and an attached channel are one fact — "this account
    /// publishes a way to reach it" — so `strict` must not reach its two-signal
    /// bar from them alone. Most people who pin a channel also link it.
    #[test]
    fn strict_does_not_count_a_channel_and_a_bio_link_separately() {
        assert!(!matched(&[F_BIO_LINK, F_PROFILE_CHANNEL], Policy::Strict));
        assert!(!matched(
            &[F_BIO_LINK, F_PROFILE_CHANNEL, F_PROFILE_OCR],
            Policy::Strict
        ));
    }

    #[test]
    fn a_ban_report_names_the_attached_channel() {
        let report = report(&[F_PROFILE_NSFW, F_PROFILE_CHANNEL]);
        let verdict = evaluate(&report, Policy::NsfwAndContact, Action::Ban, &[], false);

        assert!(verdict.reasons.contains(&F_PROFILE_CHANNEL));
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

    /// Same defect as the `no_photo_link` false positive, one preset over: a
    /// bio link and the *same* link read off the avatar are one fact, not two.
    /// A business whose logo carries its website would otherwise be banned at
    /// a 0% NSFW score.
    #[test]
    fn strict_does_not_count_contact_info_twice() {
        assert!(!matched(&[F_BIO_LINK, F_PROFILE_OCR], Policy::Strict));
    }

    #[test]
    fn the_reasons_list_only_names_signals_the_policy_used() {
        // `no_photo_link` fires, but nsfw_and_contact does not consult it, so
        // it must not appear as grounds for the ban.
        let report = report(&[F_PROFILE_NSFW, F_BIO_LINK, F_NO_PHOTO_LINK]);
        let verdict = evaluate(&report, Policy::NsfwAndContact, Action::Ban, &[], false);

        assert!(verdict.matched);
        assert!(verdict.reasons.contains(&F_PROFILE_NSFW));
        assert!(verdict.reasons.contains(&F_BIO_LINK));
        assert!(
            !verdict.reasons.contains(&F_NO_PHOTO_LINK),
            "reported a signal the policy ignored: {:?}",
            verdict.reasons
        );
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
