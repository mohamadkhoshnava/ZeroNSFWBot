use async_trait::async_trait;

use super::{F_NO_PHOTO_LINK, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::extract_contacts};

/// Fires for an account with no visible profile photo but a link in its bio.
///
/// Covers the spammer who sets their avatar to "my contacts only" precisely to
/// dodge image classification, while still needing a public bio to advertise.
/// Without this filter the image-based signals simply return "unavailable" and
/// such an account sails through.
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

        if ctx.has_photo {
            return FilterOutcome::not_triggered();
        }

        let contacts = extract_contacts(bio);
        if contacts.is_empty() {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(1.0, Some(contacts.summary(2)))
    }
}
