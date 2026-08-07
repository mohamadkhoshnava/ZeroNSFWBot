//! Text helpers shared across handlers.

use once_cell::sync::Lazy;
use regex::Regex;

/// Escape the five characters Telegram's HTML parse mode cares about.
///
/// Every piece of user-controlled text (names, usernames, bios, broadcast
/// bodies) must go through this before being interpolated into a message,
/// otherwise a display name like `<b>` breaks the whole message — or worse,
/// smuggles a link in.
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Shorten to `max` characters (not bytes), appending an ellipsis.
pub fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_owned();
    }
    let mut out: String = input.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Percentage rendered for humans: `0.784` becomes `78`.
pub fn percent(score: f32) -> u8 {
    (score.clamp(0.0, 1.0) * 100.0).round() as u8
}

static URL_RE: Lazy<Regex> = Lazy::new(|| {
    // Bare domains count: spammers write "x.com/abc" without a scheme, and
    // Telegram still linkifies it.
    Regex::new(
        r"(?ix)
        (?:https?://|www\.)\S+
        |
        \b[a-z0-9][a-z0-9-]{0,61}\.(?:com|net|org|xyz|top|club|site|online|ru|ir|info|link|live|me|cc|io|app|shop|store|fun|vip|bio|page|space|website|pro|tv)\b(?:/\S*)?
        ",
    )
    .expect("URL_RE is a valid regex")
});

static MENTION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"@[A-Za-z][A-Za-z0-9_]{4,31}").expect("MENTION_RE is a valid regex"));

static TME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:t\.me|telegram\.me|telegram\.dog|tg://)\S*")
        .expect("TME_RE is a valid regex")
});

/// Contact information found in a bio, a name, or text read off an avatar.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    pub urls: Vec<String>,
    pub mentions: Vec<String>,
    pub telegram_links: Vec<String>,
}

impl ContactInfo {
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty() && self.mentions.is_empty() && self.telegram_links.is_empty()
    }

    /// A short, already-escaped summary for the detection report.
    pub fn summary(&self, max_items: usize) -> String {
        let items: Vec<String> = self
            .telegram_links
            .iter()
            .chain(self.mentions.iter())
            .chain(self.urls.iter())
            .take(max_items)
            .map(|s| escape_html(&truncate(s, 40)))
            .collect();
        items.join(", ")
    }
}

/// Pull links, @mentions and t.me references out of arbitrary text.
pub fn extract_contacts(text: &str) -> ContactInfo {
    let telegram_links: Vec<String> = TME_RE
        .find_iter(text)
        .map(|m| m.as_str().to_owned())
        .collect();

    // A t.me link already matched above would otherwise be reported twice.
    let urls: Vec<String> = URL_RE
        .find_iter(text)
        .map(|m| m.as_str().to_owned())
        .filter(|u| {
            !telegram_links
                .iter()
                .any(|t| u.contains(t.as_str()) || t.contains(u))
        })
        .collect();

    ContactInfo {
        urls,
        mentions: MENTION_RE
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect(),
        telegram_links,
    }
}

/// Render a duration as `3d 4h 12m`, dropping zero-valued leading units.
pub fn humanize_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (d, h, m) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
    );
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(
            escape_html(r#"<b>a & "b" 'c'</b>"#),
            "&lt;b&gt;a &amp; &quot;b&quot; &#39;c&#39;&lt;/b&gt;"
        );
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        // Would panic on a byte slice: each Persian letter is 2 bytes.
        assert_eq!(truncate("سلام دنیا", 4), "سلا…");
        assert_eq!(truncate("short", 40), "short");
    }

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent(0.784), 78);
        assert_eq!(percent(1.5), 100);
        assert_eq!(percent(-0.2), 0);
    }

    #[test]
    fn finds_telegram_links() {
        let c = extract_contacts("سلام t.me/hotchannel بیا");
        assert_eq!(c.telegram_links, vec!["t.me/hotchannel"]);
    }

    #[test]
    fn finds_mentions_but_not_short_handles() {
        let c = extract_contacts("write @my_channel_18 or @ab");
        assert_eq!(c.mentions, vec!["@my_channel_18"]);
    }

    #[test]
    fn finds_schemeless_domains() {
        let c = extract_contacts("check hotstuff.xyz/join now");
        assert_eq!(c.urls, vec!["hotstuff.xyz/join"]);
    }

    #[test]
    fn does_not_double_report_telegram_links_as_urls() {
        let c = extract_contacts("https://t.me/spam");
        assert_eq!(c.telegram_links.len(), 1);
        assert!(c.urls.is_empty(), "got {:?}", c.urls);
    }

    #[test]
    fn plain_text_has_no_contacts() {
        assert!(extract_contacts("just a normal bio about cats").is_empty());
    }

    #[test]
    fn humanizes_durations() {
        assert_eq!(humanize_duration(90), "1m");
        assert_eq!(humanize_duration(3_700), "1h 1m");
        assert_eq!(humanize_duration(90_000), "1d 1h 0m");
    }
}
