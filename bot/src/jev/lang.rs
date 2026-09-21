//! Guessing a group's UI language from its title and description.
//!
//! The heuristic in [`crate::i18n::detect`] does well on script alone and
//! badly on everything else: a Persian group called "Tehran Traders" is Latin
//! script and reads as English, and Persian and Arabic share an alphabet, so a
//! title with no marker letters is decided by a coin toss that the code
//! resolves to Persian by design.
//!
//! A `choice` question over four options answers that directly, and it is
//! asked at most once per group — when the bot is added — so it costs nothing
//! in the message path. When Jev is unavailable, or unsure, the heuristic is
//! still what decides.

use serde_json::json;

use super::{Ask, JevClient, Question, clip};
use crate::i18n::Lang;

const LANGUAGE: &str = "language";

/// How sure the model must be before its answer beats the heuristic.
///
/// Getting this wrong is cheap but annoying — an Arabic group greeted in
/// Persian — and the heuristic is genuinely good whenever the script is
/// decisive, so the model only overrides it when it is clearly confident.
const MIN_CONFIDENCE: f32 = 0.6;

/// Ask which language this group is run in.
///
/// `None` means no usable answer, and the caller keeps whatever the heuristic
/// decided.
pub async fn detect(client: &JevClient, title: &str, description: Option<&str>) -> Option<Lang> {
    if !client.enabled() {
        return None;
    }

    // Nothing to read. The heuristic has its own fallback for this.
    if title.trim().is_empty() && description.is_none_or(|d| d.trim().is_empty()) {
        return None;
    }

    let mut ask = Ask::new();
    ask.add(LANGUAGE, question());

    let state = json!({
        "telegram_group": {
            "title": clip(title),
            "description": description.map(clip),
        }
    });

    let answers = client.ask(state, &ask).await?;
    let (choice, confidence) = answers.choice(LANGUAGE)?;
    if confidence < MIN_CONFIDENCE {
        tracing::debug!(choice, confidence, "Jev was unsure of the group language");
        return None;
    }

    choice.parse::<Lang>().ok()
}

fn question() -> Question {
    Question::choice(
        "Which of these languages should a moderation bot speak to this Telegram group in? \
         Judge the language the members are most likely to read, not the alphabet the title \
         happens to be typed in: a group whose title is Latin-script branding may still be run \
         in Persian, Arabic or Russian, and Persian and Arabic share an alphabet while being \
         different languages.",
        &[
            (
                "en",
                "English, or a group with no clearer signal than Latin branding.",
            ),
            (
                "fa",
                "Persian (Farsi/Dari) — Iran, Afghanistan, Tajikistan.",
            ),
            (
                "ru",
                "Russian, or another mainly Russian-speaking audience.",
            ),
            ("ar", "Arabic — any Arabic-speaking country."),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every option the model can pick has to parse back into a language the
    /// bot actually ships translations for.
    #[test]
    fn every_offered_option_is_a_language_we_have() {
        let json = serde_json::to_value(question()).unwrap();
        let options = json["criteria"].as_object().unwrap();

        assert_eq!(options.len(), Lang::ALL.len());
        for code in options.keys() {
            assert!(
                code.parse::<Lang>().is_ok(),
                "the model may answer {code:?}, which is not a supported language"
            );
        }
        for lang in Lang::ALL {
            assert!(
                options.contains_key(lang.code()),
                "{} is never offered to the model",
                lang.code()
            );
        }
    }

    #[tokio::test]
    async fn a_blank_group_is_never_asked_about() {
        let client = JevClient::new(
            "http://127.0.0.1:1/v1/systemone",
            Some("key".to_owned()),
            "jev-latest",
            std::time::Duration::from_millis(50),
        )
        .unwrap();

        assert_eq!(detect(&client, "  ", None).await, None);
        assert_eq!(detect(&client, "", Some("  ")).await, None);
    }
}
