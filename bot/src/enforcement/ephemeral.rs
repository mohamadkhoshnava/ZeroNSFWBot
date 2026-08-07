//! Optional self-cleanup of the bot's own group messages.
//!
//! Off by default: the reports are the most visible evidence that the bot is
//! working, and deleting them hides that from everyone who was not watching at
//! the moment. Admins who prefer a spotless chat can turn it on per group.

use teloxide::{
    prelude::*,
    types::{ChatId, MessageId},
};

use crate::Tg;

/// Delete `message_id` after `ttl_secs`.
///
/// Fire-and-forget: the task holds only a bot handle and two ids, and a failed
/// deletion is not worth surfacing — the message simply stays.
pub fn schedule_delete(bot: Tg, chat_id: ChatId, message_id: MessageId, ttl_secs: i32) {
    let delay = std::time::Duration::from_secs(ttl_secs.clamp(5, 86_400) as u64);

    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Err(err) = bot.delete_message(chat_id, message_id).await {
            tracing::debug!(%chat_id, %err, "could not auto-delete a bot message");
        }
    });
}
