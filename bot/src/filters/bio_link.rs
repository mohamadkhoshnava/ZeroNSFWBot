use async_trait::async_trait;

use super::{F_BIO_LINK, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::extract_contacts};

/// Fires when the bio carries a link, a t.me reference or an @username.
///
/// This is the "advertising" half of the classic NSFW-spam profile: the avatar
/// gets attention, the bio does the selling. On its own it is weak evidence —
/// plenty of legitimate people link their channel — which is why it is designed
/// to be combined with an NSFW signal rather than used alone.
pub struct BioLink;

#[async_trait]
impl Filter for BioLink {
    fn id(&self) -> &'static str {
        F_BIO_LINK
    }

    fn needs(&self) -> Needs {
        Needs {
            bio: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        // A hidden bio is unknown, not clean.
        let Some(bio) = ctx.bio.as_deref() else {
            return FilterOutcome::unavailable();
        };

        let contacts = extract_contacts(bio);
        if contacts.is_empty() {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(1.0, Some(contacts.summary(3)))
    }
}
