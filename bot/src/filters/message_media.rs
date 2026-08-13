use async_trait::async_trait;

use super::{F_MESSAGE_MEDIA, Filter, FilterOutcome, Needs};
use crate::scan::ScanContext;

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
        let Some(scoring) = ctx.message_nsfw.as_ref() else {
            return FilterOutcome::unavailable();
        };

        if scoring.score >= ctx.threshold() {
            // The detail carries both stages, so the report reads
            // "88% → 91%" rather than a bare number nobody can sanity-check.
            FilterOutcome::triggered(scoring.score, Some(scoring.detail()))
        } else {
            FilterOutcome {
                triggered: false,
                score: scoring.score,
                detail: None,
                available: true,
            }
        }
    }
}
