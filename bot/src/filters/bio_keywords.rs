use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::RegexSet;

use super::{F_BIO_KEYWORDS, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::escape_html};

/// Terms adult-content spammers put in their own bios to advertise, across the
/// four languages the bot supports plus the Latin transliterations that Persian
/// and Russian spammers commonly use.
///
/// Deliberately conservative: these must be words that are effectively never
/// innocent in a Telegram bio, because this filter can act on text alone under
/// the `nsfw_or_keywords` policy. Anything ambiguous belongs in a group's own
/// custom policy, not here.
const KEYWORD_PATTERNS: &[&str] = &[
    // English / transliterated
    r"(?i)\bp[o0]rn(?:o|hub|star)?\b",
    r"(?i)\bx+x+x+\b",
    r"(?i)\bnudes?\b",
    r"(?i)\bsex[- ]?(?:chat|cam|shop|video|tape)\b",
    r"(?i)\bonly[- ]?fans\b",
    r"(?i)\bescort\b",
    r"(?i)\bcamgirl\b",
    r"(?i)\bhentai\b",
    r"(?i)\b18\+\s*(?:content|channel|group|videos?)\b",
    r"(?i)\badult\s*(?:content|channel|videos?)\b",
    // Persian
    r"سکس|سک+س|پورن|فیلم\s*سوپر|عکس\s*لخت|شماره\s*مجازی\s*سکس",
    // Arabic
    r"سكس|إباحي|اباحي|أفلام\s*جنسية|جنس\s*ساخن",
    // Russian
    r"(?i)порн[оа]|секс[- ]?чат|интим\s*услуг|голы[ех]\s*фото",
];

static KEYWORDS: Lazy<RegexSet> =
    Lazy::new(|| RegexSet::new(KEYWORD_PATTERNS).expect("KEYWORD_PATTERNS are valid regexes"));

/// Fires when the bio, display name or username contains explicit advertising
/// vocabulary.
///
/// Text is a useful complement to the image model: a spammer can swap to a
/// clean avatar in seconds, but the wording that sells the channel has to stay.
pub struct BioKeywords;

#[async_trait]
impl Filter for BioKeywords {
    fn id(&self) -> &'static str {
        F_BIO_KEYWORDS
    }

    fn needs(&self) -> Needs {
        Needs {
            bio: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        // Name and username are always visible, so this filter can still run
        // when the bio is hidden — it just has less to work with.
        let mut haystack = ctx.display_name.clone();
        if let Some(username) = &ctx.username {
            haystack.push(' ');
            haystack.push_str(username);
        }
        if let Some(bio) = &ctx.bio {
            haystack.push(' ');
            haystack.push_str(bio);
        }

        let matched = match_indices(&haystack);
        if matched.is_empty() {
            return FilterOutcome::not_triggered();
        }

        FilterOutcome::triggered(1.0, Some(escape_html(&matched.join(", "))))
    }
}

/// The concrete substrings that matched, for the details view. Reporting the
/// matched text rather than the pattern index is what lets an admin judge a
/// false positive.
fn match_indices(text: &str) -> Vec<String> {
    KEYWORDS
        .matches(text)
        .into_iter()
        .filter_map(|idx| {
            regex::Regex::new(KEYWORD_PATTERNS[idx])
                .ok()?
                .find(text)
                .map(|m| m.as_str().to_owned())
        })
        .take(3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(text: &str) -> bool {
        KEYWORDS.is_match(text)
    }

    #[test]
    fn catches_explicit_advertising_in_each_language() {
        assert!(matches("Best porn channel here"));
        assert!(matches("کانال فیلم سوپر"));
        assert!(matches("قناة سكس"));
        assert!(matches("Секс-чат 24/7"));
        assert!(matches("XXX videos daily"));
    }

    #[test]
    fn ignores_ordinary_bios() {
        assert!(!matches("Software engineer, cat person, Tehran"));
        assert!(!matches("عاشق کتاب و سفر"));
        assert!(!matches("Люблю музыку и книги"));
        // "Essex" contains "sex" but must not match: the pattern is anchored.
        assert!(!matches("Living in Essex"));
    }

    #[test]
    fn reports_the_matched_text_not_the_pattern() {
        let found = match_indices("hey, onlyfans link in bio");
        assert_eq!(found, vec!["onlyfans"]);
    }
}
