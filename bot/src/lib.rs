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
    filters::FilterRegistry, util::admin_cache::AdminCache, util::ratelimit::RateLimiter,
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

/// Shared, immutable-after-startup application state.
pub struct App {
    pub cfg: Config,
    pub db: PgPool,
    pub detector: DetectorClient,
    pub filters: FilterRegistry,
    pub admins: AdminCache,
    /// Private-chat detector test, per user.
    pub test_limiter: RateLimiter,
    /// `(chat_id, user_id)` pairs that scanned clean recently.
    pub recent_clean: Cache<(i64, i64), ()>,
    pub broadcasts: BroadcastRegistry,
    pub bot_id: i64,
    pub started_at: DateTime<Utc>,
}

impl App {
    pub async fn new(cfg: Config, bot_id: i64) -> Result<Arc<Self>> {
        let db = db::connect(&cfg.database_url, cfg.database_max_connections).await?;
        let detector = DetectorClient::new(
            cfg.detector_url.clone(),
            cfg.detector_timeout,
            cfg.enable_ocr,
        )?;

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
            broadcasts: BroadcastRegistry::new(),
            filters: FilterRegistry::with_defaults(),
            detector,
            db,
            cfg,
            bot_id,
            started_at: Utc::now(),
        }))
    }

    pub fn uptime_secs(&self) -> i64 {
        (Utc::now() - self.started_at).num_seconds()
    }
}
