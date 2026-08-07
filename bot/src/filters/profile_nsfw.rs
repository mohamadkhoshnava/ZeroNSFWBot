use async_trait::async_trait;

use super::{F_PROFILE_NSFW, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::percent};

/// Fires when the model's NSFW probability for the user's profile photos
/// reaches the group's threshold.
///
/// The score is the maximum across the scanned photos, which is what covers the
/// pinned-plus-previous case: Telegram returns the displayed photo at index 0
/// and the rest in reverse-chronological order, so taking the max means a
/// spammer cannot hide behind a clean pinned avatar.
pub struct ProfileNsfw;

#[async_trait]
impl Filter for ProfileNsfw {
    fn id(&self) -> &'static str {
        F_PROFILE_NSFW
    }

    fn needs(&self) -> Needs {
        Needs {
            profile_photos: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        let Some(score) = ctx.profile_nsfw else {
            // No photo, or the photo could not be fetched or decoded.
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
