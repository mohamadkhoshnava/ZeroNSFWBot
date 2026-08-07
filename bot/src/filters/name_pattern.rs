use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;

use super::{F_NAME_PATTERN, Filter, FilterOutcome, Needs};
use crate::{scan::ScanContext, util::text::escape_html};

/// Structural tells in the display name itself, independent of vocabulary.
///
/// Adult spam accounts routinely carry the advertisement in the name, since it
/// is the one field shown next to every comment: an invite link, a "18+"
/// marker, or a call to action with an arrow pointing at the avatar.
static PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "invite link in name",
            Regex::new(r"(?i)(?:t\.me/|@[A-Za-z][A-Za-z0-9_]{4,})").unwrap(),
        ),
        (
            "age marker in name",
            Regex::new(r"(?:\b18\s*\+|\+\s*18\b|🔞)").unwrap(),
        ),
        (
            "call to action in name",
            Regex::new(r"(?i)(?:click|join|watch|subscribe|بزن|ببین|عکسمو|اضغط|شاهد|смотри|жми)\b")
                .unwrap(),
        ),
        (
            "pointer emoji in name",
            Regex::new(r"[\u{1F447}\u{1F446}\u{1F449}\u{1F448}\u{2B06}\u{2B07}]").unwrap(),
        ),
    ]
});

/// Fires on names or usernames shaped like an advertisement.
///
/// A single hit is weak on its own — an emoji-heavy display name is not a
/// crime — so this filter is intended to be combined, never used alone.
pub struct NamePattern;

#[async_trait]
impl Filter for NamePattern {
    fn id(&self) -> &'static str {
        F_NAME_PATTERN
    }

    fn needs(&self) -> Needs {
        // Names arrive with the message itself; nothing extra to fetch.
        Needs::default()
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        let hits: Vec<&str> = PATTERNS
            .iter()
            .filter(|(_, re)| re.is_match(&ctx.display_name))
            .map(|(label, _)| *label)
            .collect();

        if hits.is_empty() {
            return FilterOutcome::not_triggered();
        }

        // Two independent tells in one name is a much stronger signal than one.
        let score = if hits.len() >= 2 { 1.0 } else { 0.6 };
        FilterOutcome::triggered(score, Some(escape_html(&hits.join(", "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(name: &str) -> usize {
        PATTERNS.iter().filter(|(_, re)| re.is_match(name)).count()
    }

    #[test]
    fn flags_advertising_names() {
        assert!(hits("Anna 🔞 @hot_channel_x") >= 2);
        assert!(hits("👇 click here 👇") >= 2);
        assert!(hits("عکسمو ببین 👇") >= 1);
    }

    #[test]
    fn leaves_ordinary_names_alone() {
        assert_eq!(hits("Seyed"), 0);
        assert_eq!(hits("Мария Иванова"), 0);
        assert_eq!(hits("علی رضایی"), 0);
        assert_eq!(hits("Ali 🌸"), 0);
    }
}
