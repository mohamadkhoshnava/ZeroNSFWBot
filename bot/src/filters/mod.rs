//! The extensible signal layer.
//!
//! Each [`Filter`] answers one narrow question about a user ("is the avatar
//! NSFW?", "does the bio advertise a channel?"). Filters never perform network
//! calls of their own: [`crate::scan::ScanContext`] gathers everything once, so
//! adding a filter costs no extra Telegram or detector traffic.
//!
//! # Adding a filter
//!
//! 1. Create `src/filters/my_filter.rs` implementing [`Filter`].
//! 2. Add its id to the `filter_ids!` list below.
//! 3. Register it in [`FilterRegistry::with_defaults`].
//! 4. Add a `reason_<id>` key to all four locale files.
//!
//! It becomes selectable in `/nsfw → Filter mode → Custom` automatically.

mod bio_keywords;
mod bio_link;
mod bio_semantic;
mod message_media;
mod name_pattern;
mod no_photo_link;
mod profile_channel;
mod profile_nsfw;
mod profile_ocr;
mod reputation;

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::scan::ScanContext;

/// Declares the ids once and derives the `ALL_FILTERS` slice from the same
/// list, so a new filter cannot be half-registered.
macro_rules! filter_ids {
    ($($konst:ident => $value:literal),+ $(,)?) => {
        $(pub const $konst: &str = $value;)+
        pub const ALL_FILTERS: &[&str] = &[$($value),+];
    };
}

filter_ids! {
    F_PROFILE_NSFW  => "profile_nsfw",
    F_MESSAGE_MEDIA => "message_media",
    F_BIO_LINK      => "bio_link",
    F_PROFILE_CHANNEL => "profile_channel",
    F_BIO_KEYWORDS  => "bio_keywords",
    F_BIO_SEMANTIC  => "bio_semantic",
    F_NAME_PATTERN  => "name_pattern",
    F_PROFILE_OCR   => "profile_ocr",
    F_NO_PHOTO_LINK => "no_photo_link",
    F_REPUTATION    => "reputation",
}

/// Signals that are not profile filters and never appear in a policy.
///
/// They belong to the message-text scan, which works like the group media
/// scan: it judges one message on its own terms rather than asking what kind
/// of account sent it, so it builds its own verdict instead of going through
/// [`crate::policy::evaluate`]. The ids exist so the report, the details view
/// and the `reason_*` translations work exactly as they do for a filter —
/// but they are deliberately outside `filter_ids!`, because offering them in
/// the custom-policy picker would list two signals that can never fire there.
pub const F_TEXT_TOPIC: &str = "text_topic";
pub const F_TEXT_AD: &str = "text_ad";

/// The data a filter needs. The scanner fetches the union of the needs of the
/// filters the active policy actually consults, and nothing more.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    pub profile_photos: bool,
    pub bio: bool,
    /// The channel attached to the profile. Shares one `getChat` with `bio`,
    /// so asking for both costs exactly one call.
    pub personal_chat: bool,
    pub avatar_text: bool,
    pub message_media: bool,
    pub reputation: bool,
    /// Jev's reading of the profile text. Its own flag so a group whose policy
    /// never consults `bio_semantic` makes no API call at all.
    pub profile_ad: bool,
}

impl Needs {
    pub fn merge(self, other: Self) -> Self {
        Self {
            profile_photos: self.profile_photos || other.profile_photos,
            bio: self.bio || other.bio,
            personal_chat: self.personal_chat || other.personal_chat,
            // Reading text off the avatar implies downloading the avatar.
            avatar_text: self.avatar_text || other.avatar_text,
            message_media: self.message_media || other.message_media,
            reputation: self.reputation || other.reputation,
            profile_ad: self.profile_ad || other.profile_ad,
        }
    }
}

/// What one filter concluded about one user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterOutcome {
    pub triggered: bool,
    /// Confidence in `0.0..=1.0`. For the NSFW filters this is the model's
    /// probability; for boolean filters it is 0.0 or 1.0.
    pub score: f32,
    /// Short, already HTML-escaped evidence shown in the details view.
    pub detail: Option<String>,
    /// `false` when the filter could not run — a hidden bio, say. Distinct from
    /// "ran and found nothing", because a policy that *requires* this filter
    /// must not silently treat unavailable as clean.
    pub available: bool,
}

impl FilterOutcome {
    pub fn not_triggered() -> Self {
        Self {
            triggered: false,
            score: 0.0,
            detail: None,
            available: true,
        }
    }

    pub fn unavailable() -> Self {
        Self {
            triggered: false,
            score: 0.0,
            detail: None,
            available: false,
        }
    }

    pub fn triggered(score: f32, detail: Option<String>) -> Self {
        Self {
            triggered: true,
            score: score.clamp(0.0, 1.0),
            detail,
            available: true,
        }
    }
}

#[async_trait]
pub trait Filter: Send + Sync {
    /// Stable id; also the `reason_<id>` translation key and the value stored
    /// in `groups.custom_filters`.
    fn id(&self) -> &'static str;

    fn needs(&self) -> Needs;

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome;
}

/// The verdict inputs for one user: every filter's outcome, keyed by id.
#[derive(Debug, Default, Clone)]
pub struct ScanReport {
    outcomes: HashMap<&'static str, FilterOutcome>,
}

impl ScanReport {
    pub fn get(&self, id: &str) -> Option<&FilterOutcome> {
        self.outcomes.get(id)
    }

    /// `true` only when the filter ran *and* fired. An unavailable filter is
    /// never treated as a match.
    pub fn triggered(&self, id: &str) -> bool {
        self.outcomes.get(id).is_some_and(|o| o.triggered)
    }

    pub fn score(&self, id: &str) -> f32 {
        self.outcomes.get(id).map_or(0.0, |o| o.score)
    }

    pub fn triggered_ids(&self) -> Vec<&'static str> {
        // Sorted so reports and tests are deterministic despite the HashMap.
        let mut ids: Vec<&'static str> = self
            .outcomes
            .iter()
            .filter(|(_, o)| o.triggered)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    pub fn iter(&self) -> impl Iterator<Item = (&&'static str, &FilterOutcome)> {
        self.outcomes.iter()
    }

    /// The headline number shown to users: the strongest NSFW probability seen,
    /// whether it came from the profile or from the message's own media.
    pub fn headline_score(&self) -> f32 {
        self.score(F_PROFILE_NSFW).max(self.score(F_MESSAGE_MEDIA))
    }

    pub fn insert(&mut self, id: &'static str, outcome: FilterOutcome) {
        self.outcomes.insert(id, outcome);
    }
}

pub struct FilterRegistry {
    filters: Vec<Arc<dyn Filter>>,
}

impl FilterRegistry {
    pub fn with_defaults() -> Self {
        Self {
            filters: vec![
                Arc::new(profile_nsfw::ProfileNsfw),
                Arc::new(message_media::MessageMedia),
                Arc::new(bio_link::BioLink),
                Arc::new(profile_channel::ProfileChannel),
                Arc::new(bio_keywords::BioKeywords),
                Arc::new(bio_semantic::BioSemantic),
                Arc::new(name_pattern::NamePattern),
                Arc::new(profile_ocr::ProfileOcr),
                Arc::new(no_photo_link::NoPhotoLink),
                Arc::new(reputation::Reputation),
            ],
        }
    }

    pub fn register(&mut self, filter: Arc<dyn Filter>) {
        self.filters.push(filter);
    }

    /// Union of the needs of the filters in `wanted`.
    pub fn needs_for(&self, wanted: &[&str]) -> Needs {
        self.filters
            .iter()
            .filter(|f| wanted.contains(&f.id()))
            .fold(Needs::default(), |acc, f| acc.merge(f.needs()))
    }

    /// Run every filter concurrently and collect the outcomes.
    ///
    /// All of them run, not just the policy-relevant ones: the extra cost is
    /// pure CPU over already-fetched data, and the details view is far more
    /// useful when an admin can see the signals their policy ignored.
    pub async fn evaluate(&self, ctx: &ScanContext) -> ScanReport {
        let outcomes = futures::future::join_all(
            self.filters
                .iter()
                .map(|f| async move { (f.id(), f.evaluate(ctx).await) }),
        )
        .await;

        ScanReport {
            outcomes: outcomes.into_iter().collect(),
        }
    }
}

impl Default for FilterRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_id_is_registered() {
        let registry = FilterRegistry::with_defaults();
        let registered: Vec<&str> = registry.filters.iter().map(|f| f.id()).collect();

        for id in ALL_FILTERS {
            assert!(
                registered.contains(id),
                "filter {id} is declared but not registered"
            );
        }
        assert_eq!(
            registered.len(),
            ALL_FILTERS.len(),
            "a filter is registered twice"
        );
    }

    #[test]
    fn needs_are_scoped_to_the_requested_filters() {
        let registry = FilterRegistry::with_defaults();

        let bio_only = registry.needs_for(&[F_BIO_LINK]);
        assert!(bio_only.bio);
        assert!(
            !bio_only.profile_photos,
            "a bio filter must not force a photo download"
        );

        let with_ocr = registry.needs_for(&[F_PROFILE_OCR]);
        assert!(with_ocr.avatar_text && with_ocr.profile_photos);
    }

    #[test]
    fn unavailable_outcomes_never_count_as_triggered() {
        let mut report = ScanReport::default();
        report.insert(F_BIO_LINK, FilterOutcome::unavailable());

        assert!(!report.triggered(F_BIO_LINK));
        assert!(report.triggered_ids().is_empty());
    }
}
