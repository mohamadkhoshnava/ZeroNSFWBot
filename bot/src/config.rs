//! Process configuration, loaded once from the environment at startup.

use std::{collections::HashSet, str::FromStr, time::Duration};

use anyhow::{Context, Result, bail};

use crate::i18n::Lang;
use crate::policy::{Action, Policy};

/// Where the source lives, unless `PROJECT_URL` says otherwise.
const DEFAULT_PROJECT_URL: &str = "https://github.com/mohamadkhoshnava/ZeroNSFWBot";

/// The maintainer's public channel, credited in the bot's own messages.
const DEFAULT_DEVELOPER_CHANNEL: &str = "@SEYED_BAX";

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_token: String,
    pub bot_username: String,
    pub bot_name: String,
    pub super_admins: HashSet<i64>,
    /// Public source repository, linked from the private chat. Overridable so
    /// a fork points at its own repo rather than upstream.
    pub project_url: String,
    /// Maintainer's channel, credited at the foot of the bot's own messages.
    /// Set it empty to drop the credit line entirely.
    pub developer_channel: String,

    pub database_url: String,
    pub database_max_connections: u32,

    pub detector_url: String,
    pub detector_timeout: Duration,
    pub enable_ocr: bool,
    /// Largest clip the bot will download to sample frames from. Anything
    /// bigger falls back to Telegram's thumbnail — one arbitrary frame, but
    /// free. 20 MB is also the ceiling `getFile` will serve.
    pub media_max_bytes: u32,

    pub defaults: GroupDefaults,

    pub scan_cache_ttl: Duration,
    pub admin_cache_ttl: Duration,
    pub test_mode_rate_per_min: u32,
    pub broadcast_rate_per_sec: u32,
    pub global_reputation_min_bans: i64,
}

/// Seed values for a group the bot has just been added to. Once a group row
/// exists, its own columns win — these are never re-applied.
#[derive(Debug, Clone)]
pub struct GroupDefaults {
    pub lang: Lang,
    /// Auto-ban threshold as a percentage, 0-100.
    pub threshold: i16,
    pub policy: Policy,
    pub action: Action,
    pub profile_photos_to_scan: usize,
    pub dry_run: bool,
    pub grace_messages: i32,
    pub delete_bot_messages: bool,
    pub bot_message_ttl_secs: i32,

    /// Whether new groups start with the group media scan on. Off: it looks at
    /// every member's every picture, which is a decision an admin should make
    /// rather than inherit.
    pub media_scan: bool,
    /// Percent, 0-100. Much higher than `threshold` by design — see
    /// `migrations/0004_group_media_scan.sql`.
    pub media_threshold: i16,
    pub media_action: Action,
    /// Stills sampled per animation or clip.
    pub media_frames: i16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bot_token = req("TELEGRAM_BOT_TOKEN")?;
        if bot_token.contains("ExampleToken") {
            bail!(
                "TELEGRAM_BOT_TOKEN is still the placeholder from .env.example — \
                 put your real @BotFather token in .env"
            );
        }

        let super_admins = opt("SUPER_ADMINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i64>()
                    .with_context(|| format!("SUPER_ADMINS contains a non-numeric id: {s:?}"))
            })
            .collect::<Result<HashSet<_>>>()?;

        if super_admins.is_empty() {
            tracing::warn!(
                "SUPER_ADMINS is empty — /info and /broadcast will be unavailable to everyone"
            );
        }

        let threshold = num("DEFAULT_THRESHOLD", 40_i16)?;
        if !(0..=100).contains(&threshold) {
            bail!("DEFAULT_THRESHOLD must be between 0 and 100, got {threshold}");
        }

        let media_threshold = num("DEFAULT_MEDIA_THRESHOLD", 90_i16)?;
        if !(0..=100).contains(&media_threshold) {
            bail!("DEFAULT_MEDIA_THRESHOLD must be between 0 and 100, got {media_threshold}");
        }

        Ok(Self {
            bot_username: req("BOT_USERNAME")?.trim_start_matches('@').to_owned(),
            bot_name: opt("BOT_NAME").unwrap_or_else(|| "NSFW Guard".to_owned()),
            bot_token,
            super_admins,
            project_url: opt("PROJECT_URL").unwrap_or_else(|| DEFAULT_PROJECT_URL.to_owned()),
            // `opt` treats blank as unset, so an operator who wants no credit
            // line must remove the variable rather than empty it. Honour the
            // explicit empty value here instead.
            developer_channel: std::env::var("DEVELOPER_CHANNEL")
                .unwrap_or_else(|_| DEFAULT_DEVELOPER_CHANNEL.to_owned())
                .trim()
                .to_owned(),

            database_url: req("DATABASE_URL")?,
            database_max_connections: num("DATABASE_MAX_CONNECTIONS", 10)?,

            detector_url: opt("DETECTOR_URL")
                .unwrap_or_else(|| "http://detector:8000".to_owned())
                .trim_end_matches('/')
                .to_owned(),
            detector_timeout: Duration::from_secs(num("DETECTOR_TIMEOUT_SECS", 20)?),
            enable_ocr: flag("ENABLE_OCR", true),
            media_max_bytes: num::<u32>("MEDIA_MAX_DOWNLOAD_BYTES", 20 * 1024 * 1024)?,

            defaults: GroupDefaults {
                lang: parsed("DEFAULT_LANG", Lang::En)?,
                threshold,
                policy: parsed("DEFAULT_POLICY", Policy::NsfwAndContact)?,
                action: parsed("DEFAULT_ACTION", Action::Ban)?,
                profile_photos_to_scan: num::<usize>("PROFILE_PHOTOS_TO_SCAN", 2)?.clamp(1, 10),
                dry_run: flag("DEFAULT_DRY_RUN", false),
                grace_messages: num("DEFAULT_GRACE_MESSAGES", 5)?,
                delete_bot_messages: flag("DEFAULT_AUTO_DELETE_BOT_MESSAGES", false),
                bot_message_ttl_secs: num("DEFAULT_BOT_MESSAGE_TTL_SECS", 60)?,
                media_scan: flag("DEFAULT_MEDIA_SCAN", false),
                media_threshold,
                media_action: parsed("DEFAULT_MEDIA_ACTION", Action::Delete)?,
                media_frames: num::<i16>("DEFAULT_MEDIA_FRAMES", 5)?
                    .clamp(1, crate::db::models::MAX_MEDIA_FRAMES),
            },

            scan_cache_ttl: Duration::from_secs(num("SCAN_CACHE_TTL_SECS", 86_400)?),
            admin_cache_ttl: Duration::from_secs(num("ADMIN_CACHE_TTL_SECS", 300)?),
            test_mode_rate_per_min: num("TEST_MODE_RATE_PER_MIN", 10)?,
            broadcast_rate_per_sec: num("BROADCAST_RATE_PER_SEC", 25)?.max(1),
            global_reputation_min_bans: num("GLOBAL_REPUTATION_MIN_BANS", 3)?,
        })
    }

    pub fn is_super_admin(&self, user_id: i64) -> bool {
        self.super_admins.contains(&user_id)
    }

    /// Deep link that opens the "add me to a group" chat picker.
    pub fn add_to_group_url(&self) -> String {
        format!("https://t.me/{}?startgroup=true", self.bot_username)
    }

    /// The credit line appended to the bot's own messages, or empty when
    /// `DEVELOPER_CHANNEL` is set to nothing.
    pub fn credit_line(&self, lang: Lang) -> String {
        if self.developer_channel.is_empty() {
            return String::new();
        }
        format!(
            "\n\n{}",
            crate::t!(
                lang,
                "developer_channel",
                channel = crate::util::text::escape_html(&self.developer_channel)
            )
        )
    }
}

fn req(key: &str) -> Result<String> {
    let value = std::env::var(key)
        .with_context(|| format!("required environment variable {key} is not set"))?;
    if value.trim().is_empty() {
        bail!("required environment variable {key} is empty");
    }
    Ok(value)
}

fn opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn num<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match opt(key) {
        None => Ok(default),
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("{key} is not a valid number ({raw:?}): {e}")),
    }
}

fn flag(key: &str, default: bool) -> bool {
    match opt(key) {
        None => default,
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
    }
}

fn parsed<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match opt(key) {
        None => Ok(default),
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("{key} has an unsupported value ({raw:?}): {e}")),
    }
}
