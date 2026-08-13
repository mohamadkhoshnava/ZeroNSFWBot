//! Translation catalog and message lookup.
//!
//! The four locale files are embedded at compile time, so there is no runtime
//! file I/O and a missing locale is a build failure rather than a 3am panic.

mod detect;
mod lang;

use std::collections::HashMap;

use once_cell::sync::Lazy;

pub use detect::{detect_group_lang, script_profile};
pub use lang::Lang;

const EN: &str = include_str!("../../locales/en.toml");
const FA: &str = include_str!("../../locales/fa.toml");
const RU: &str = include_str!("../../locales/ru.toml");
const AR: &str = include_str!("../../locales/ar.toml");

type Catalog = HashMap<String, String>;

static CATALOGS: Lazy<HashMap<&'static str, Catalog>> = Lazy::new(|| {
    Lang::ALL
        .iter()
        .map(|&lang| {
            let raw = match lang {
                Lang::En => EN,
                Lang::Fa => FA,
                Lang::Ru => RU,
                Lang::Ar => AR,
            };
            let parsed: Catalog = toml::from_str(raw)
                .unwrap_or_else(|e| panic!("locale {}.toml is not valid TOML: {e}", lang.code()));
            (lang.code(), parsed)
        })
        .collect()
});

/// Look up a message, falling back to English and then to the key itself.
///
/// Returning the key rather than panicking means a translation gap degrades
/// into an ugly message instead of a dead handler.
pub fn lookup(lang: Lang, key: &str) -> &'static str {
    fn get(lang: Lang, key: &str) -> Option<&'static str> {
        CATALOGS
            .get(lang.code())
            .and_then(|c| c.get(key))
            .map(|s| s.as_str())
    }

    get(lang, key)
        .or_else(|| {
            tracing::warn!(lang = lang.code(), key, "missing translation");
            get(Lang::En, key)
        })
        .unwrap_or_else(|| {
            // Leak is bounded by the number of distinct keys in the binary.
            Box::leak(format!("⟦{key}⟧").into_boxed_str())
        })
}

/// Look up a message without falling back or warning.
///
/// For keys built from data the bot does not control — a model's class names,
/// say — where a miss is expected and the caller has a better fallback than
/// `⟦key⟧`.
pub fn lookup_opt(lang: Lang, key: &str) -> Option<&'static str> {
    CATALOGS
        .get(lang.code())
        .and_then(|c| c.get(key))
        .or_else(|| CATALOGS.get(Lang::En.code()).and_then(|c| c.get(key)))
        .map(|s| s.as_str())
}

/// All keys defined for a language. Used by the completeness test.
pub fn keys(lang: Lang) -> Vec<&'static str> {
    CATALOGS
        .get(lang.code())
        .map(|c| c.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default()
}

/// Substitute `{name}` placeholders in an already-resolved template.
pub fn fill(template: &str, args: &[(&str, String)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// `t!(lang, "key")` or `t!(lang, "key", count = 3, name = who)`.
///
/// Values are stringified with `Display`; escape untrusted text with
/// [`crate::util::text::escape_html`] *before* passing it in.
#[macro_export]
macro_rules! t {
    ($lang:expr, $key:expr) => {
        $crate::i18n::lookup($lang, $key).to_string()
    };
    ($lang:expr, $key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::lookup($lang, $key),
            &[$((stringify!($name), $value.to_string())),+],
        )
    };
}
