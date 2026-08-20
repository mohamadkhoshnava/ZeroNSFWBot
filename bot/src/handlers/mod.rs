//! Update routing.

pub mod botguard;
pub mod callback;
pub mod commands;
pub mod group;
pub mod member;
pub mod private;

use teloxide::{dispatching::UpdateHandler, prelude::*};

use crate::handlers::commands::Command;

/// The dispatcher tree.
///
/// Order matters: commands are matched first so `/nsfw` in a group is handled
/// as a command rather than scanned as an ordinary comment.
pub fn schema() -> UpdateHandler<anyhow::Error> {
    let messages = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(commands::handle),
        )
        .branch(
            dptree::filter(|msg: Message| msg.chat.is_private()).endpoint(private::handle_message),
        )
        .branch(dptree::endpoint(group::handle_message));

    dptree::entry()
        .branch(messages)
        .branch(Update::filter_callback_query().endpoint(callback::handle))
        .branch(Update::filter_my_chat_member().endpoint(member::handle_my_chat_member))
        // Someone else's membership changing. Registering this is what makes
        // teloxide ask Telegram for `chat_member` updates at all, which is the
        // only way to see a bot that arrived through an invite link rather than
        // being added by a member.
        .branch(Update::filter_chat_member().endpoint(botguard::on_chat_member))
}
