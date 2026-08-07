use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// The languages the bot ships translations for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Fa,
    Ru,
    Ar,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::En, Lang::Fa, Lang::Ru, Lang::Ar];

    pub const fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fa => "fa",
            Lang::Ru => "ru",
            Lang::Ar => "ar",
        }
    }

    /// Name of the language written in that language, for the picker.
    pub const fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fa => "فارسی",
            Lang::Ru => "Русский",
            Lang::Ar => "العربية",
        }
    }

    pub const fn flag(self) -> &'static str {
        match self {
            Lang::En => "🇬🇧",
            Lang::Fa => "🇮🇷",
            Lang::Ru => "🇷🇺",
            Lang::Ar => "🇸🇦",
        }
    }

    pub const fn is_rtl(self) -> bool {
        matches!(self, Lang::Fa | Lang::Ar)
    }

    /// Parse a Telegram `language_code` such as `fa`, `fa-IR` or `ru-RU`.
    /// Unknown languages fall back to English rather than failing.
    pub fn from_telegram_code(code: &str) -> Lang {
        let primary = code.split(['-', '_']).next().unwrap_or("");
        match primary.to_ascii_lowercase().as_str() {
            "fa" | "pes" | "prs" => Lang::Fa,
            "ru" => Lang::Ru,
            "ar" => Lang::Ar,
            // Telegram has no Dari/Tajik UI, but these users read Persian.
            "tg" => Lang::Fa,
            _ => Lang::En,
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Lang {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" => Ok(Lang::En),
            "fa" => Ok(Lang::Fa),
            "ru" => Ok(Lang::Ru),
            "ar" => Ok(Lang::Ar),
            other => Err(format!(
                "unknown language {other:?} (supported: en, fa, ru, ar)"
            )),
        }
    }
}

/// Lenient conversion used when reading a value back out of the database:
/// a corrupted row must not take the bot down.
impl From<&str> for Lang {
    fn from(s: &str) -> Self {
        s.parse().unwrap_or_default()
    }
}
