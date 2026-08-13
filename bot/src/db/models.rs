//! Row types.
//!
//! Queries are runtime-checked (`query_as`) rather than macro-checked, so the
//! Docker build does not need a live database to compile.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::{
    config::GroupDefaults,
    i18n::Lang,
    policy::{Action, Policy},
};

/// A group's moderation configuration, as stored.
///
/// `lang`, `policy` and `action` are `TEXT` in Postgres and parsed leniently
/// here: a value written by a newer version of the bot must degrade to the
/// default rather than take the handler down.
#[derive(Debug, Clone, FromRow)]
pub struct GroupRow {
    pub chat_id: i64,
    pub title: Option<String>,
    pub username: Option<String>,
    pub lang: String,
    pub lang_locked: bool,
    pub threshold: i16,
    pub policy: String,
    pub custom_filters: serde_json::Value,
    pub nsfw_categories: serde_json::Value,
    pub action: String,
    pub dry_run: bool,
    pub grace_messages: i32,
    pub delete_bot_messages: bool,
    pub bot_message_ttl_secs: i32,
    pub global_blocklist: bool,
    pub media_scan: bool,
    pub media_threshold: i16,
    pub media_action: String,
    pub media_kinds: serde_json::Value,
    pub media_frames: i16,
    pub member_count: i32,
    pub is_active: bool,
    pub added_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Classes counted as NSFW when a group has never chosen.
///
/// `sexy` and `drawings` are deliberately out: they are the two buckets that
/// made the single-model pipeline unusable, and a group that wants them can say
/// so explicitly.
pub fn default_nsfw_categories() -> Vec<String> {
    vec!["porn".to_owned(), "hentai".to_owned()]
}

/// Ceiling on the frames sampled per clip.
///
/// Every frame is a full model inference, so this bounds what one forwarded
/// video can cost. The detector clamps it again on its own side.
pub const MAX_MEDIA_FRAMES: i16 = 12;

/// A kind of media the group media scan can be pointed at.
///
/// Separate switches rather than one "scan media" flag because their costs are
/// nothing alike: a photo is one small download, a video can be 20 MB, and a
/// group that wants stickers checked but not every clip anyone forwards should
/// be able to say exactly that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Photo,
    /// GIFs. Telegram re-encodes them to soundless MP4, so they are sampled
    /// like video rather than read as GIF bytes.
    Animation,
    Sticker,
    Video,
}

impl MediaKind {
    pub const ALL: [MediaKind; 4] = [
        MediaKind::Photo,
        MediaKind::Animation,
        MediaKind::Sticker,
        MediaKind::Video,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Animation => "animation",
            MediaKind::Sticker => "sticker",
            MediaKind::Video => "video",
        }
    }

    /// Translation key for the human-readable name.
    pub const fn label_key(self) -> &'static str {
        match self {
            MediaKind::Photo => "media_kind_photo",
            MediaKind::Animation => "media_kind_animation",
            MediaKind::Sticker => "media_kind_sticker",
            MediaKind::Video => "media_kind_video",
        }
    }
}

impl std::fmt::Display for MediaKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MediaKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "photo" => Ok(MediaKind::Photo),
            "animation" | "gif" => Ok(MediaKind::Animation),
            "sticker" => Ok(MediaKind::Sticker),
            "video" => Ok(MediaKind::Video),
            other => Err(format!("unknown media kind {other:?}")),
        }
    }
}

/// Kinds scanned when a group has never chosen: everything.
///
/// Turning the section on is already a deliberate act, so it starts by
/// covering what an admin would expect "scan the media in my group" to mean,
/// and narrowing it is one tap away.
pub fn default_media_kinds() -> Vec<MediaKind> {
    MediaKind::ALL.to_vec()
}

/// The parsed, ready-to-use view of a [`GroupRow`].
#[derive(Debug, Clone)]
pub struct GroupSettings {
    pub chat_id: i64,
    pub title: Option<String>,
    pub lang: Lang,
    pub lang_locked: bool,
    pub threshold: i16,
    pub policy: Policy,
    pub custom_filters: Vec<String>,
    /// Which of the verifier's classes this group treats as NSFW. Empty means
    /// no image can ever be explicit, which the panel warns about.
    pub nsfw_categories: Vec<String>,
    pub action: Action,
    pub dry_run: bool,
    pub grace_messages: i32,
    pub delete_bot_messages: bool,
    pub bot_message_ttl_secs: i32,
    pub global_blocklist: bool,

    /// Whether media posted in the group is scanned in its own right, rather
    /// than only as one signal about a newcomer's account.
    pub media_scan: bool,
    /// Percent. Deliberately its own number: this one convicts an image on its
    /// own, with no bio or avatar to corroborate it.
    pub media_threshold: i16,
    /// What to do about explicit media, independent of the profile action.
    pub media_action: Action,
    pub media_kinds: Vec<MediaKind>,
    /// Stills sampled across an animation or clip.
    pub media_frames: i16,
}

impl GroupSettings {
    /// Threshold as the `0.0..=1.0` probability the filters compare against.
    pub fn threshold_ratio(&self) -> f32 {
        f32::from(self.threshold.clamp(0, 100)) / 100.0
    }

    /// Media threshold as a `0.0..=1.0` probability.
    pub fn media_threshold_ratio(&self) -> f32 {
        f32::from(self.media_threshold.clamp(0, 100)) / 100.0
    }

    pub fn scans_media_kind(&self, kind: MediaKind) -> bool {
        self.media_scan && self.media_kinds.contains(&kind)
    }

    /// In-memory defaults, used when a group row does not exist yet.
    pub fn defaults(chat_id: i64, defaults: &GroupDefaults) -> Self {
        Self {
            chat_id,
            title: None,
            lang: defaults.lang,
            lang_locked: false,
            threshold: defaults.threshold,
            policy: defaults.policy,
            custom_filters: Vec::new(),
            nsfw_categories: default_nsfw_categories(),
            action: defaults.action,
            dry_run: defaults.dry_run,
            grace_messages: defaults.grace_messages,
            delete_bot_messages: defaults.delete_bot_messages,
            bot_message_ttl_secs: defaults.bot_message_ttl_secs,
            global_blocklist: true,
            media_scan: defaults.media_scan,
            media_threshold: defaults.media_threshold,
            media_action: defaults.media_action,
            media_kinds: default_media_kinds(),
            media_frames: defaults.media_frames,
        }
    }
}

impl From<GroupRow> for GroupSettings {
    fn from(row: GroupRow) -> Self {
        Self {
            chat_id: row.chat_id,
            title: row.title,
            lang: row.lang.as_str().into(),
            lang_locked: row.lang_locked,
            threshold: row.threshold.clamp(0, 100),
            policy: row.policy.parse().unwrap_or_default(),
            custom_filters: serde_json::from_value(row.custom_filters).unwrap_or_default(),
            nsfw_categories: serde_json::from_value(row.nsfw_categories)
                .unwrap_or_else(|_| default_nsfw_categories()),
            action: row.action.parse().unwrap_or_default(),
            dry_run: row.dry_run,
            grace_messages: row.grace_messages.max(0),
            delete_bot_messages: row.delete_bot_messages,
            bot_message_ttl_secs: row.bot_message_ttl_secs.clamp(5, 86_400),
            global_blocklist: row.global_blocklist,
            media_scan: row.media_scan,
            media_threshold: row.media_threshold.clamp(0, 100),
            media_action: row.media_action.parse().unwrap_or(Action::Delete),
            // Parsed one entry at a time, and unknown entries dropped: a kind
            // written by a newer version must cost that one kind, not the whole
            // list. An unreadable column means "scan nothing", never "scan
            // everything" — the lenient reading of a broken setting must not be
            // the more destructive one.
            media_kinds: serde_json::from_value::<Vec<String>>(row.media_kinds)
                .unwrap_or_default()
                .iter()
                .filter_map(|name| name.parse().ok())
                .collect(),
            media_frames: row.media_frames.clamp(1, MAX_MEDIA_FRAMES),
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub lang: String,
    pub lang_locked: bool,
    pub started_bot: bool,
    pub is_blocked: bool,
}

impl UserRow {
    pub fn lang(&self) -> Lang {
        self.lang.as_str().into()
    }
}

/// One entry of a detection's evidence, persisted as JSONB and replayed in the
/// "Details" view long after the scan itself is gone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredReason {
    pub filter: String,
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A detection to be written to the log.
#[derive(Debug, Clone)]
pub struct NewDetection {
    pub chat_id: i64,
    pub user_id: i64,
    pub message_id: Option<i32>,
    pub score: f32,
    pub verdict: Action,
    pub banned: bool,
    pub deleted: bool,
    pub muted: bool,
    pub dry_run: bool,
    pub reasons: Vec<StoredReason>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DetectionRow {
    pub id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub score: f32,
    pub banned: bool,
    pub reasons: serde_json::Value,
}

impl DetectionRow {
    pub fn reasons(&self) -> Vec<StoredReason> {
        serde_json::from_value(self.reasons.clone()).unwrap_or_default()
    }
}

/// Counts over one time window, for the `/nsfw → Statistics` view.
#[derive(Debug, Clone, Copy, Default, FromRow)]
pub struct PeriodStats {
    pub detected: i64,
    pub deleted: i64,
    pub banned: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatsBundle {
    pub day: PeriodStats,
    pub week: PeriodStats,
    pub month: PeriodStats,
    pub all: PeriodStats,
}

impl StatsBundle {
    pub fn is_empty(&self) -> bool {
        self.all.detected == 0
    }
}

/// Bot-wide numbers for `/info`.
#[derive(Debug, Clone, Default)]
pub struct GlobalStats {
    pub users: i64,
    pub active_users: i64,
    pub groups: i64,
    pub active_groups: i64,
    pub members: i64,
    pub detections: StatsBundle,
    pub bans_day: i64,
    pub bans_week: i64,
    pub bans_month: i64,
    pub bans_all: i64,
}

/// A cached profile scan, valid only while the photo fingerprint matches.
///
/// Deliberately group-independent. The cache is keyed by user and shared across
/// every group the account appears in, so it stores the *evidence* — the
/// screening score and the verifier's per-class breakdown — and each group
/// derives its own verdict from that using its own chosen categories.
#[derive(Debug, Clone, FromRow)]
pub struct ScanCacheRow {
    pub photo_fingerprint: String,
    /// What the screening model said. Group-independent.
    pub nsfw_score: f32,
    pub has_photo: bool,
    pub bio: Option<String>,
    pub ocr_text: Option<String>,
    /// The verifier's per-class probabilities, or `NULL` when the account was
    /// screened but never escalated — some group's threshold was high enough
    /// that the second stage did not run.
    pub verifier_labels: Option<serde_json::Value>,
}

impl ScanCacheRow {
    /// The verifier's breakdown, strongest class first.
    pub fn labels(&self) -> Option<Vec<(String, f32)>> {
        let map: std::collections::HashMap<String, f32> =
            serde_json::from_value(self.verifier_labels.clone()?).ok()?;
        let mut out: Vec<(String, f32)> = map.into_iter().collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(media_kinds: serde_json::Value, media_frames: i16, media_action: &str) -> GroupRow {
        let now = Utc::now();
        GroupRow {
            chat_id: -1001,
            title: None,
            username: None,
            lang: "en".into(),
            lang_locked: false,
            threshold: 40,
            policy: "nsfw_and_contact".into(),
            custom_filters: serde_json::json!([]),
            nsfw_categories: serde_json::json!(["porn"]),
            action: "ban".into(),
            dry_run: false,
            grace_messages: 5,
            delete_bot_messages: false,
            bot_message_ttl_secs: 60,
            global_blocklist: true,
            media_scan: true,
            media_threshold: 90,
            media_action: media_action.into(),
            media_kinds,
            media_frames,
            member_count: 0,
            is_active: true,
            added_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn stored_media_kinds_are_parsed() {
        let settings: GroupSettings =
            row(serde_json::json!(["photo", "video"]), 5, "delete").into();

        assert_eq!(
            settings.media_kinds,
            vec![MediaKind::Photo, MediaKind::Video]
        );
    }

    /// A kind written by a newer version must cost that one kind, not the whole
    /// list: dropping the list would silently switch the section off in a group
    /// that believes it is protected.
    #[test]
    fn an_unknown_media_kind_does_not_discard_the_known_ones() {
        let settings: GroupSettings = row(
            serde_json::json!(["photo", "hologram", "video"]),
            5,
            "delete",
        )
        .into();

        assert_eq!(
            settings.media_kinds,
            vec![MediaKind::Photo, MediaKind::Video]
        );
    }

    /// The lenient reading of a broken setting must never be the more
    /// destructive one. An unreadable list scans nothing, rather than deleting
    /// on the strength of a column nobody can parse.
    #[test]
    fn an_unreadable_media_kind_list_scans_nothing() {
        let settings: GroupSettings = row(serde_json::json!("not-a-list"), 5, "delete").into();

        assert!(settings.media_kinds.is_empty());
        for kind in MediaKind::ALL {
            assert!(!settings.scans_media_kind(kind));
        }
    }

    #[test]
    fn the_frame_count_is_clamped_to_something_affordable() {
        let settings: GroupSettings = row(serde_json::json!(["photo"]), 9_999, "delete").into();
        assert_eq!(settings.media_frames, MAX_MEDIA_FRAMES);

        let settings: GroupSettings = row(serde_json::json!(["photo"]), 0, "delete").into();
        assert_eq!(settings.media_frames, 1, "zero frames would scan nothing");
    }

    /// An action string this build does not know must not become `ban` by way
    /// of `Action::default()`. The media section's default is the mild one.
    #[test]
    fn an_unknown_media_action_falls_back_to_deleting_not_banning() {
        let settings: GroupSettings = row(serde_json::json!(["photo"]), 5, "vaporise").into();

        assert_eq!(settings.media_action, Action::Delete);
    }

    #[test]
    fn a_known_media_action_is_honoured() {
        let settings: GroupSettings = row(serde_json::json!(["photo"]), 5, "ban").into();
        assert_eq!(settings.media_action, Action::Ban);
    }
}
