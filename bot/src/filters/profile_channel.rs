use async_trait::async_trait;

use super::{F_PROFILE_CHANNEL, Filter, FilterOutcome, Needs};
use crate::{
    scan::{PersonalChannel, ScanContext},
    util::text::{escape_html, truncate},
};

/// Fires when the account has a channel attached to its profile.
///
/// Telegram lets a user pin a channel to their profile, where it is displayed
/// next to the bio. It is the same advertising as a `t.me/…` in the bio, but
/// [`super::F_BIO_LINK`] cannot see it: the bio text stays empty or innocent
/// while every visitor to the profile still gets the channel. Spam accounts
/// with a clean bio and an attached channel were passing `nsfw_and_contact`
/// untouched for exactly this reason.
///
/// Like `bio_link`, this is weak evidence alone — plenty of ordinary people
/// attach their own channel — so it belongs on the advertising side of an
/// NSFW-plus-contact decision, never on its own.
pub struct ProfileChannel;

#[async_trait]
impl Filter for ProfileChannel {
    fn id(&self) -> &'static str {
        F_PROFILE_CHANNEL
    }

    fn needs(&self) -> Needs {
        Needs {
            personal_chat: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        match &ctx.personal_channel {
            // A failed or skipped getChat is unknown, not clean.
            PersonalChannel::Unknown => FilterOutcome::unavailable(),
            PersonalChannel::Absent => FilterOutcome::not_triggered(),
            PersonalChannel::Linked(channel) => {
                FilterOutcome::triggered(1.0, Some(escape_html(&truncate(&channel.label(), 40))))
            }
        }
    }
}
