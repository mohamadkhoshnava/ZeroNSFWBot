//! ZeroNSFWBot — detects and removes accounts that advertise NSFW content
//! through their profile, bio, username, avatar or the media they post.
//!
//! Everything is exposed as a library so the integration tests can drive the
//! same code the binary runs.

pub mod admin;
pub mod config;
pub mod db;
pub mod detector;
pub mod enforcement;
pub mod filters;
pub mod handlers;
pub mod i18n;
pub mod jev;
pub mod media;
pub mod policy;
pub mod scan;
pub mod ui;
pub mod util;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Utc};
use moka::future::Cache;
use sqlx::PgPool;
use teloxide::{
    Bot,
    adaptors::{DefaultParseMode, Throttle},
};

use crate::{
    admin::broadcast::BroadcastRegistry, config::Config, detector::DetectorClient,
    filters::FilterRegistry, jev::JevClient, util::admin_cache::AdminCache,
    util::ratelimit::RateLimiter,
};

/// The bot handle used everywhere.
///
/// `Throttle` keeps the bot inside Telegram's rate limits without every call
/// site thinking about it; `DefaultParseMode` means messages are HTML by
/// default, which is why all interpolated user text is escaped.
pub type Tg = DefaultParseMode<Throttle<Bot>>;

/// How long a user who scanned clean is skipped in the same group.
///
/// Bounds the cost of scanning every message: without it a chatty new member
/// would trigger a `getUserProfilePhotos` and a `getChat` per message. Short
/// enough that swapping to an NSFW avatar is caught within minutes.
const CLEAN_USER_TTL: Duration = Duration::from_secs(15 * 60);

/// How long a scored piece of media is remembered.
///
/// Long, because unlike a user an image cannot change: `file_unique_id`
/// identifies exact bytes. The ceiling is memory, not staleness.
const MEDIA_SCORE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Shared, immutable-after-startup application state.
pub struct App {
    pub cfg: Config,
    pub db: PgPool,
    pub detector: DetectorClient,
    /// Typed text decisions. Disabled, and harmless, without an API key.
    pub jev: JevClient,
    pub filters: FilterRegistry,
    pub admins: AdminCache,
    /// Private-chat detector test, per user.
    pub test_limiter: RateLimiter,
    /// `(chat_id, user_id)` pairs that scanned clean recently.
    pub recent_clean: Cache<(i64, i64), ()>,
    /// Scores for media already seen, keyed by Telegram's `file_unique_id`.
    ///
    /// Bounded by content rather than by user: the same sticker, meme and GIF
    /// circulate endlessly, and that id is stable forever, so a popular sticker
    /// is downloaded and scored once instead of on every post.
    pub media_scores: Cache<String, crate::scan::ImageScoring>,
    pub broadcasts: BroadcastRegistry,
    pub bot_id: i64,
    pub started_at: DateTime<Utc>,
    /// The verifier's classes, discovered from the detector at startup. Empty
    /// when no verifier is loaded, which the categories panel reports rather
    /// than offering a list that cannot take effect.
    pub verifier_categories: Vec<String>,
}

impl App {
    pub async fn new(cfg: Config, bot_id: i64) -> Result<Arc<Self>> {
        let db = db::connect(&cfg.database_url, cfg.database_max_connections).await?;
        let detector = DetectorClient::new(
            cfg.detector_url.clone(),
            cfg.detector_timeout,
            cfg.enable_ocr,
        )?;
        let jev = JevClient::new(
            cfg.jev_url.clone(),
            cfg.jev_api_key.clone(),
            cfg.jev_model.clone(),
            cfg.jev_timeout,
        )?;

        // Best effort: a detector that is still loading its model must not stop
        // the bot from starting. The panel says so, and a restart picks it up.
        let verifier_categories = match detector.models().await {
            Ok(models) => models.verifier.map(|v| v.labels).unwrap_or_default(),
            Err(err) => {
                tracing::warn!(%err, "could not read the detector's model list");
                Vec::new()
            }
        };

        Ok(Arc::new(Self {
            admins: AdminCache::new(cfg.admin_cache_ttl),
            test_limiter: RateLimiter::new(cfg.test_mode_rate_per_min, Duration::from_secs(60)),
            recent_clean: Cache::builder()
                .time_to_live(CLEAN_USER_TTL)
                .max_capacity(100_000)
                // Lets a settings change drop just that group's entries, so
                // tuning the threshold takes effect on the next message
                // instead of up to CLEAN_USER_TTL later.
                .support_invalidation_closures()
                .build(),
            media_scores: Cache::builder()
                .time_to_live(MEDIA_SCORE_TTL)
                .max_capacity(50_000)
                .build(),
            broadcasts: BroadcastRegistry::new(),
            filters: FilterRegistry::with_defaults(),
            detector,
            jev,
            db,
            cfg,
            bot_id,
            started_at: Utc::now(),
            verifier_categories,
        }))
    }

    pub fn uptime_secs(&self) -> i64 {
        (Utc::now() - self.started_at).num_seconds()
    }
}
