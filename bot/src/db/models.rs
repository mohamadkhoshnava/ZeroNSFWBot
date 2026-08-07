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
    pub action: String,
    pub dry_run: bool,
    pub grace_messages: i32,
    pub delete_bot_messages: bool,
    pub bot_message_ttl_secs: i32,
    pub global_blocklist: bool,
    pub member_count: i32,
    pub is_active: bool,
    pub added_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    pub action: Action,
    pub dry_run: bool,
    pub grace_messages: i32,
    pub delete_bot_messages: bool,
    pub bot_message_ttl_secs: i32,
    pub global_blocklist: bool,
}

impl GroupSettings {
    /// Threshold as the `0.0..=1.0` probability the filters compare against.
    pub fn threshold_ratio(&self) -> f32 {
        f32::from(self.threshold.clamp(0, 100)) / 100.0
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
            action: defaults.action,
            dry_run: defaults.dry_run,
            grace_messages: defaults.grace_messages,
            delete_bot_messages: defaults.delete_bot_messages,
            bot_message_ttl_secs: defaults.bot_message_ttl_secs,
            global_blocklist: true,
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
            action: row.action.parse().unwrap_or_default(),
            dry_run: row.dry_run,
            grace_messages: row.grace_messages.max(0),
            delete_bot_messages: row.delete_bot_messages,
            bot_message_ttl_secs: row.bot_message_ttl_secs.clamp(5, 86_400),
            global_blocklist: row.global_blocklist,
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
#[derive(Debug, Clone, FromRow)]
pub struct ScanCacheRow {
    pub photo_fingerprint: String,
    pub nsfw_score: f32,
    pub has_photo: bool,
    pub bio: Option<String>,
    pub ocr_text: Option<String>,
}
