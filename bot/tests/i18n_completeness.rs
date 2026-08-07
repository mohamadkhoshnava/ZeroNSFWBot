//! Guards the translations.
//!
//! A missing key degrades gracefully at runtime (English, then the raw key),
//! but a missing *placeholder* silently ships a message with a literal
//! `{score}` in it. Both are caught here instead of in production.

use std::collections::{HashMap, HashSet};

use zeronsfw_bot::i18n::{self, Lang};

fn key_set(lang: Lang) -> HashSet<&'static str> {
    i18n::keys(lang).into_iter().collect()
}

/// The `{placeholder}` names a template expects.
fn placeholders(text: &str) -> HashSet<String> {
    let mut found = HashSet::new();
    let mut rest = text;

    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            found.insert(name.to_owned());
        }
        rest = &rest[start + end + 1..];
    }

    found
}

#[test]
fn every_locale_defines_the_same_keys() {
    let english = key_set(Lang::En);
    assert!(english.len() > 50, "the English catalog looks truncated");

    for lang in Lang::ALL {
        let keys = key_set(lang);

        let missing: Vec<_> = english.difference(&keys).collect();
        assert!(
            missing.is_empty(),
            "{}.toml is missing {} key(s): {missing:?}",
            lang.code(),
            missing.len()
        );

        let extra: Vec<_> = keys.difference(&english).collect();
        assert!(
            extra.is_empty(),
            "{}.toml defines key(s) absent from en.toml: {extra:?}",
            lang.code()
        );
    }
}

#[test]
fn placeholders_match_the_english_originals() {
    let mut problems: Vec<String> = Vec::new();

    for key in i18n::keys(Lang::En) {
        let expected = placeholders(i18n::lookup(Lang::En, key));

        for lang in Lang::ALL {
            if lang == Lang::En {
                continue;
            }
            let actual = placeholders(i18n::lookup(lang, key));

            // Extra placeholders would render literally; missing ones drop data
            // the message was supposed to carry.
            if actual != expected {
                problems.push(format!(
                    "{}.toml key {key:?}: expected {expected:?}, found {actual:?}",
                    lang.code()
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "placeholder mismatches:\n{}",
        problems.join("\n")
    );
}

#[test]
fn no_translation_is_left_empty() {
    for lang in Lang::ALL {
        for key in i18n::keys(lang) {
            assert!(
                !i18n::lookup(lang, key).trim().is_empty(),
                "{}.toml key {key:?} is empty",
                lang.code()
            );
        }
    }
}

#[test]
fn every_filter_and_policy_has_a_label_in_every_language() {
    use zeronsfw_bot::{
        filters::ALL_FILTERS,
        policy::{Action, Policy},
    };

    let mut required: Vec<String> = ALL_FILTERS
        .iter()
        .map(|id| format!("reason_{id}"))
        .collect();
    required.extend(
        Policy::ALL
            .iter()
            .flat_map(|p| [p.label_key().to_owned(), p.desc_key().to_owned()]),
    );
    required.extend(Action::ALL.iter().map(|a| a.label_key().to_owned()));

    for lang in Lang::ALL {
        let keys = key_set(lang);
        for key in &required {
            assert!(
                keys.contains(key.as_str()),
                "{}.toml is missing {key:?}, which the UI renders unconditionally",
                lang.code()
            );
        }
    }
}

#[test]
fn missing_keys_fall_back_instead_of_panicking() {
    let rendered = i18n::lookup(Lang::Fa, "definitely_not_a_real_key");
    assert!(rendered.contains("definitely_not_a_real_key"));
}

#[test]
fn fill_substitutes_named_placeholders() {
    let out = i18n::fill(
        "{a} and {b}, not {a}",
        &[("a", "x".into()), ("b", "y".into())],
    );
    assert_eq!(out, "x and y, not x");
}

#[test]
fn html_in_templates_is_balanced() {
    // An unbalanced tag makes Telegram reject the whole message with a 400,
    // which would look like the bot silently doing nothing.
    for lang in Lang::ALL {
        for key in i18n::keys(lang) {
            let text = i18n::lookup(lang, key);
            for tag in ["b", "i", "code", "a"] {
                let opens = text.matches(&format!("<{tag}>")).count()
                    + text.matches(&format!("<{tag} ")).count();
                let closes = text.matches(&format!("</{tag}>")).count();
                assert_eq!(
                    opens,
                    closes,
                    "{}.toml key {key:?} has unbalanced <{tag}> tags",
                    lang.code()
                );
            }
        }
    }
}

#[test]
fn language_codes_are_unique_and_parse_back() {
    let mut seen: HashMap<&str, Lang> = HashMap::new();
    for lang in Lang::ALL {
        assert!(
            seen.insert(lang.code(), lang).is_none(),
            "duplicate language code {}",
            lang.code()
        );
        assert_eq!(lang.code().parse::<Lang>().unwrap(), lang);
    }
}
