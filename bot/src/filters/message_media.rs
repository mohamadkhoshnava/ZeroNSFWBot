use async_trait::async_trait;

use super::{F_MESSAGE_MEDIA, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::percent};

/// Fires when the photo, sticker or animation *inside the comment* is NSFW.
///
/// Separate from the profile filter because a clean account posting explicit
/// media and an explicit account posting an emoji are different problems, and
/// admins reasonably want to treat them differently.
pub struct MessageMedia;

#[async_trait]
impl Filter for MessageMedia {
    fn id(&self) -> &'static str {
        F_MESSAGE_MEDIA
    }

    fn needs(&self) -> Needs {
        Needs {
            message_media: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        let Some(score) = ctx.message_nsfw else {
            return FilterOutcome::unavailable();
        };

        if score >= ctx.threshold() {
            FilterOutcome::triggered(score, Some(format!("{}%", percent(score))))
        } else {
            FilterOutcome {
                triggered: false,
                score,
                detail: None,
                available: true,
            }
        }
    }
}
