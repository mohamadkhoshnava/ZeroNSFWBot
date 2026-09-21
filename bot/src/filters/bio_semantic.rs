use async_trait::async_trait;

use super::{F_BIO_SEMANTIC, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::percent};

/// How certain the model must be before this counts as a signal.
///
/// Deliberately fixed rather than following the group's threshold. That number
/// governs an image score, where 40% is one signal among several that a policy
/// then has to corroborate; this filter can convict on text alone under
/// `nsfw_or_keywords`, so it is held to the bar that suits its own rubric. At
/// 0.7 the model has to have placed the profile at least at "promotes
/// something adult without saying so plainly", not merely "suggestive".
const BAR: f32 = 0.7;

/// Fires when the model reads the profile text as adult advertising.
///
/// The counterpart to [`super::bio_keywords`], which matches a fixed word list.
/// That list is the floor — it runs with no API key and cannot be talked out of
/// firing — and this is the ceiling: it reads obfuscated spellings, this
/// month's slang, and the bios that advertise without using a listed word at
/// all. When Jev is not configured this filter reports itself unavailable, so
/// nothing in the bot changes.
pub struct BioSemantic;

#[async_trait]
impl Filter for BioSemantic {
    fn id(&self) -> &'static str {
        F_BIO_SEMANTIC
    }

    fn needs(&self) -> Needs {
        Needs {
            bio: true,
            // The attached channel is the single most telling field — an empty
            // bio next to a channel called "Hot Videos 🔥" is the whole spam
            // pattern — and it shares its `getChat` with the bio, so asking for
            // it costs nothing extra.
            personal_chat: true,
            profile_ad: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        // No key, nothing substantive to read, or the request failed. All three
        // are "unknown", never "clean".
        let Some(score) = ctx.profile_ad else {
            return FilterOutcome::unavailable();
        };

        if score < BAR {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(score, Some(format!("{}%", percent(score))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::GroupDefaults, db::models::GroupSettings};

    fn ctx(profile_ad: Option<f32>) -> ScanContext {
        let settings = GroupSettings::defaults(-1001, &GroupDefaults::default());
        let mut ctx = ScanContext::media_only(
            42,
            "Anna".to_owned(),
            None,
            settings,
            crate::scan::ImageScoring::screened(0.0),
            3,
        );
        ctx.profile_ad = profile_ad;
        ctx
    }

    #[tokio::test]
    async fn no_answer_is_unavailable_rather_than_clean() {
        let outcome = BioSemantic.evaluate(&ctx(None)).await;
        assert!(!outcome.available, "a Jev outage must not read as a pass");
        assert!(!outcome.triggered);
    }

    #[tokio::test]
    async fn a_suggestive_profile_alone_does_not_fire() {
        let outcome = BioSemantic.evaluate(&ctx(Some(0.5))).await;
        assert!(outcome.available);
        assert!(!outcome.triggered);
    }

    #[tokio::test]
    async fn a_confident_advertisement_fires_and_shows_its_number() {
        let outcome = BioSemantic.evaluate(&ctx(Some(0.91))).await;
        assert!(outcome.triggered);
        assert_eq!(outcome.detail.as_deref(), Some("91%"));
    }
}
