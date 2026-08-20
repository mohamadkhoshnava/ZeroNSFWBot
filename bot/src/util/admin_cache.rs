//! Cached `getChatAdministrators` lookups.
//!
//! Every `/nsfw` press and every detection report needs the admin list. Without
//! a cache that is one API call per event, which burns the per-chat rate limit
//! long before the moderation calls do.

use std::{sync::Arc, time::Duration};

use moka::future::Cache;
use teloxide::{prelude::*, types::ChatId};

use crate::Tg;

/// A group's administrators, plus what the bot itself is allowed to do there.
#[derive(Debug, Clone, Default)]
pub struct AdminSnapshot {
    pub admin_ids: Vec<i64>,
    /// Administrators that are themselves bots, kept apart from `admin_ids` so
    /// `is_admin` keeps meaning "a human who can moderate here". The bot guard
    /// needs the distinction: a promoted bot is one an admin chose to have.
    pub admin_bot_ids: Vec<i64>,
    /// Ids that can change bot settings: owner plus admins who can restrict.
    pub bot_is_admin: bool,
    pub bot_can_delete: bool,
    pub bot_can_restrict: bool,
}

impl AdminSnapshot {
    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_ids.contains(&user_id)
    }

    /// Whether this bot was promoted here — the one thing that separates a bot
    /// the group wanted from one a spammer dropped in.
    pub fn is_admin_bot(&self, user_id: i64) -> bool {
        self.admin_bot_ids.contains(&user_id)
    }
}

#[derive(Clone)]
pub struct AdminCache {
    inner: Cache<i64, Arc<AdminSnapshot>>,
}

impl AdminCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Cache::builder()
                .time_to_live(ttl)
                .max_capacity(20_000)
                .build(),
        }
    }

    pub async fn get(&self, bot: &Tg, chat_id: ChatId, bot_id: i64) -> Arc<AdminSnapshot> {
        if let Some(hit) = self.inner.get(&chat_id.0).await {
            return hit;
        }

        let snapshot = Arc::new(fetch(bot, chat_id, bot_id).await);
        self.inner.insert(chat_id.0, snapshot.clone()).await;
        snapshot
    }

    /// Drop the cached entry so the next lookup re-reads from Telegram. Called
    /// after the bot's own rights change.
    pub async fn invalidate(&self, chat_id: ChatId) {
        self.inner.invalidate(&chat_id.0).await;
    }
}

async fn fetch(bot: &Tg, chat_id: ChatId, bot_id: i64) -> AdminSnapshot {
    let members = match bot.get_chat_administrators(chat_id).await {
        Ok(members) => members,
        Err(err) => {
            // Losing this list must not block moderation: an empty snapshot
            // degrades to "nobody is an admin and I have no rights", which the
            // callers already report to the group.
            tracing::warn!(%chat_id, %err, "could not fetch chat administrators");
            return AdminSnapshot::default();
        }
    };

    let mut snapshot = AdminSnapshot::default();
    for member in &members {
        let id = member.user.id.0 as i64;
        if id == bot_id {
            snapshot.bot_is_admin = true;
            snapshot.bot_can_delete = member.kind.can_delete_messages();
            snapshot.bot_can_restrict = member.kind.can_restrict_members();
        } else if member.user.is_bot {
            snapshot.admin_bot_ids.push(id);
        } else {
            snapshot.admin_ids.push(id);
        }
    }
    snapshot
}
