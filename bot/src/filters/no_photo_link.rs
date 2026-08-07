use async_trait::async_trait;

use super::{F_NO_PHOTO_LINK, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::extract_contacts};

/// Fires for an account with positively no visible profile photo, but a link
/// in its bio.
///
/// # Why this is not in any preset
///
/// It was originally an escape hatch in `nsfw_and_contact`, for a spammer who
/// hides their avatar to dodge the classifier. That was a mistake, and it
/// banned a real person with a 0% NSFW score: "no avatar and a link in the bio"
/// describes an enormous number of ordinary, privacy-conscious Telegram users,
/// and it contains no evidence of NSFW content whatsoever.
///
/// It is also not independent of [`super::F_BIO_LINK`] — it *requires* a bio
/// link — so counting both as separate signals double-counts one fact.
///
/// It survives because it is a genuine signal when an admin deliberately pairs
/// it with something else under a custom policy (with `bio_keywords`, say).
/// It must never again be enough on its own.
pub struct NoPhotoLink;

#[async_trait]
impl Filter for NoPhotoLink {
    fn id(&self) -> &'static str {
        F_NO_PHOTO_LINK
    }

    fn needs(&self) -> Needs {
        Needs {
            profile_photos: true,
            bio: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        let Some(bio) = ctx.bio.as_deref() else {
            return FilterOutcome::unavailable();
        };

        // Only a positive "Telegram says there are no photos" counts. A skipped
        // or failed lookup is unknown, and treating it as "no photo" is exactly
        // how a transient API error turns into a ban.
        if !ctx.photo_is_absent() {
            return FilterOutcome::not_triggered();
        }

        let contacts = extract_contacts(bio);
        if contacts.is_empty() {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(1.0, Some(contacts.summary(2)))
    }
}
