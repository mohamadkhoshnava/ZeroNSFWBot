use async_trait::async_trait;

use super::{F_PROFILE_NSFW, Filter, FilterOutcome, Needs};
use crate::scan::ScanContext;

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
        let Some(scoring) = ctx.profile_nsfw.as_ref() else {
            // No photo, the photo could not be fetched or decoded, or it was flagged
            // and the verifier could not confirm it.
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
