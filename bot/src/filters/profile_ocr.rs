use async_trait::async_trait;

use super::{F_PROFILE_OCR, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::extract_contacts};

/// Fires when contact information is written *on* the avatar image.
///
/// This is the workaround spammers use once bio filtering becomes common: the
/// @username or t.me link is rendered into the profile picture, where no text
/// field contains it. OCR is what closes that hole.
pub struct ProfileOcr;

#[async_trait]
impl Filter for ProfileOcr {
    fn id(&self) -> &'static str {
        F_PROFILE_OCR
    }

    fn needs(&self) -> Needs {
        Needs {
            profile_photos: true,
            avatar_text: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        // OCR disabled, no avatar, or nothing legible on it.
        let Some(text) = ctx.avatar_text.as_deref() else {
            return FilterOutcome::unavailable();
        };

        let contacts = extract_contacts(text);
        if contacts.is_empty() {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(1.0, Some(contacts.summary(2)))
    }
}
