//! Bot-wide announcements, restricted to `SUPER_ADMINS`.
//!
//! Sending is deliberately two-step — compose, then confirm from a preview —
//! because a broadcast reaches every group and every private user at once and
//! cannot be recalled.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use moka::future::Cache;
use teloxide::{prelude::*, types::ChatId};

use crate::{App, Tg, db, enforcement::handle_dm_failure, i18n::Lang, t};

/// Who a broadcast goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Users,
    Groups,
    All,
}

impl Target {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "users" | "pm" | "private" => Target::Users,
            "groups" | "chats" => Target::Groups,
            "all" | "both" | "everyone" => Target::All,
            _ => return None,
        })
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Target::Users => "broadcast_target_users",
            Target::Groups => "broadcast_target_groups",
            Target::All => "broadcast_target_all",
        }
    }
}

/// A composed but unconfirmed broadcast.
#[derive(Debug, Clone)]
pub struct Pending {
    pub target: Target,
    pub body: String,
    pub recipients: Vec<i64>,
}

/// Pending broadcasts, keyed by the super-admin who composed them, plus a
/// single global "one at a time" latch.
#[derive(Clone)]
pub struct BroadcastRegistry {
    pending: Cache<i64, Arc<Pending>>,
    running: Arc<AtomicBool>,
}

impl BroadcastRegistry {
    pub fn new() -> Self {
        Self {
            // A preview left unconfirmed expires rather than lingering until
            // the next restart.
            pending: Cache::builder()
                .time_to_live(Duration::from_secs(10 * 60))
                .max_capacity(64)
                .build(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn stage(&self, admin_id: i64, pending: Pending) {
        self.pending.insert(admin_id, Arc::new(pending)).await;
    }

    pub async fn take(&self, admin_id: i64) -> Option<Arc<Pending>> {
        let pending = self.pending.get(&admin_id).await;
        self.pending.invalidate(&admin_id).await;
        pending
    }

    pub async fn discard(&self, admin_id: i64) {
        self.pending.invalidate(&admin_id).await;
    }

    /// Claim the single broadcast slot. `false` means one is already running.
    fn claim(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn release(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

impl Default for BroadcastRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the recipient list for a target.
pub async fn recipients(app: &Arc<App>, target: Target) -> Result<Vec<i64>> {
    Ok(match target {
        Target::Users => db::users::reachable_ids(&app.db).await?,
        Target::Groups => db::groups::active_chat_ids(&app.db).await?,
        Target::All => {
            let mut ids = db::users::reachable_ids(&app.db).await?;
            ids.extend(db::groups::active_chat_ids(&app.db).await?);
            ids
        }
    })
}

/// Deliver a staged broadcast, pacing the sends and reporting progress.
///
/// Runs to completion in the caller's task; the command handler spawns it so
/// the admin's chat stays responsive.
pub async fn run(
    app: Arc<App>,
    bot: Tg,
    admin_id: i64,
    lang: Lang,
    pending: Arc<Pending>,
) -> Result<()> {
    if !app.broadcasts.claim() {
        bot.send_message(ChatId(admin_id), t!(lang, "broadcast_busy"))
            .await?;
        return Ok(());
    }

    let total = pending.recipients.len();
    let progress = bot
        .send_message(
            ChatId(admin_id),
            t!(lang, "broadcast_started", count = total),
        )
        .await?;

    let broadcast_id = start_record(&app, admin_id, &pending, total as i32).await;

    // Telegram's bot-wide ceiling is ~30 messages/second; staying under it is
    // cheaper than absorbing the 429s that follow.
    let gap = Duration::from_secs_f64(1.0 / f64::from(app.cfg.broadcast_rate_per_sec));
    let mut ticker = tokio::time::interval(gap);

    let (mut sent, mut failed) = (0_i32, 0_i32);
    for (index, chat_id) in pending.recipients.iter().enumerate() {
        ticker.tick().await;

        match bot.send_message(ChatId(*chat_id), &pending.body).await {
            Ok(_) => sent += 1,
            Err(err) => {
                failed += 1;
                // A user id here means a private chat, so a block is worth
                // recording; group failures usually mean the bot was removed.
                if *chat_id > 0 {
                    handle_dm_failure(&app, *chat_id, &err).await;
                } else if is_gone(&err) {
                    let _ = db::groups::set_active(&app.db, *chat_id, false).await;
                }
            }
        }

        // Editing every send would itself hit the rate limit.
        if index % 100 == 99 {
            let _ = bot
                .edit_message_text(
                    ChatId(admin_id),
                    progress.id,
                    t!(
                        lang,
                        "broadcast_progress",
                        sent = sent,
                        count = total,
                        failed = failed
                    ),
                )
                .await;
        }
    }

    finish_record(&app, broadcast_id, sent, failed).await;
    app.broadcasts.release();

    bot.edit_message_text(
        ChatId(admin_id),
        progress.id,
        t!(lang, "broadcast_done", sent = sent, failed = failed),
    )
    .await?;

    Ok(())
}

/// Did Telegram say this chat is unreachable for good?
fn is_gone(err: &teloxide::RequestError) -> bool {
    matches!(
        err,
        teloxide::RequestError::Api(
            teloxide::ApiError::BotKicked
                | teloxide::ApiError::BotKickedFromSupergroup
                | teloxide::ApiError::ChatNotFound
                | teloxide::ApiError::GroupDeactivated
        )
    )
}

async fn start_record(app: &Arc<App>, admin_id: i64, pending: &Pending, total: i32) -> Option<i64> {
    let target = match pending.target {
        Target::Users => "users",
        Target::Groups => "groups",
        Target::All => "all",
    };

    sqlx::query_as::<_, (i64,)>(
        "INSERT INTO broadcasts (author_id, target, body, total) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(admin_id)
    .bind(target)
    .bind(&pending.body)
    .bind(total)
    .fetch_one(&app.db)
    .await
    .inspect_err(|err| tracing::warn!(%err, "could not record the broadcast"))
    .ok()
    .map(|row| row.0)
}

async fn finish_record(app: &Arc<App>, id: Option<i64>, sent: i32, failed: i32) {
    let Some(id) = id else { return };

    let _ = sqlx::query(
        "UPDATE broadcasts SET sent = $2, failed = $3, finished_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(sent)
    .bind(failed)
    .execute(&app.db)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_aliases() {
        assert_eq!(Target::parse("users"), Some(Target::Users));
        assert_eq!(Target::parse("PM"), Some(Target::Users));
        assert_eq!(Target::parse(" groups "), Some(Target::Groups));
        assert_eq!(Target::parse("all"), Some(Target::All));
        assert_eq!(Target::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn only_one_broadcast_can_run_at_a_time() {
        let registry = BroadcastRegistry::new();

        assert!(registry.claim());
        assert!(
            !registry.claim(),
            "second claim must fail while one is running"
        );

        registry.release();
        assert!(registry.claim(), "slot must be reusable after release");
    }

    #[tokio::test]
    async fn taking_a_pending_broadcast_consumes_it() {
        let registry = BroadcastRegistry::new();
        registry
            .stage(
                7,
                Pending {
                    target: Target::All,
                    body: "hi".into(),
                    recipients: vec![1, 2],
                },
            )
            .await;

        assert!(registry.take(7).await.is_some());
        assert!(
            registry.take(7).await.is_none(),
            "a confirmation must not be replayable"
        );
    }
}
