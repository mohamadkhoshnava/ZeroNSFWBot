//! Guess a group's language from its title and description.
//!
//! Script detection does most of the work. The hard case is Persian vs Arabic,
//! which share a script: those are separated by codepoints that only one of
//! them uses (Persian `گ چ پ ژ ک ی` and `۰۹`, Arabic `ة ي ك ى` and `٠٩`) plus a
//! small keyword list. When the evidence is inconclusive the tie goes to
//! Persian, as configured.

use super::Lang;

/// Character counts per script family for a piece of text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScriptProfile {
    pub latin: usize,
    pub cyrillic: usize,
    pub arabic_script: usize,
    pub persian_markers: usize,
    pub arabic_markers: usize,
}

impl ScriptProfile {
    pub fn total_letters(&self) -> usize {
        self.latin + self.cyrillic + self.arabic_script
    }
}

/// Codepoints used in Persian but not in standard Arabic orthography.
const PERSIAN_CHARS: &[char] = &['پ', 'چ', 'ژ', 'گ', 'ک', 'ی'];
/// Codepoints used in Arabic but not in Persian orthography.
const ARABIC_CHARS: &[char] = &['ة', 'ي', 'ك', 'ى', 'إ', 'أ', 'ؤ', 'ئ'];

/// Common words that settle the Persian/Arabic split when the letters do not.
const PERSIAN_WORDS: &[&str] = &[
    "گروه",
    "کانال",
    "چت",
    "گپ",
    "ایران",
    "فارسی",
    "خبر",
    "بحث",
    "نظرات",
    "دیدگاه",
    "تبادل",
    "دوستان",
    "برنامه",
    "آموزش",
];
const ARABIC_WORDS: &[&str] = &[
    "قناة",
    "مجموعة",
    "دردشة",
    "العربية",
    "أخبار",
    "تعليقات",
    "نقاش",
    "الأصدقاء",
    "مصر",
    "السعودية",
    "العراق",
];

pub fn script_profile(text: &str) -> ScriptProfile {
    let mut p = ScriptProfile::default();

    for ch in text.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' => p.latin += 1,
            // Cyrillic block plus its supplement.
            '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}' => p.cyrillic += 1,
            // Arabic, Arabic Supplement, Extended-A and Presentation Forms.
            '\u{0600}'..='\u{06FF}'
            | '\u{0750}'..='\u{077F}'
            | '\u{08A0}'..='\u{08FF}'
            | '\u{FB50}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFF}' => {
                p.arabic_script += 1;
                if PERSIAN_CHARS.contains(&ch) || ('\u{06F0}'..='\u{06F9}').contains(&ch) {
                    p.persian_markers += 1;
                } else if ARABIC_CHARS.contains(&ch) || ('\u{0660}'..='\u{0669}').contains(&ch) {
                    p.arabic_markers += 1;
                }
            }
            _ => {}
        }
    }

    for word in PERSIAN_WORDS {
        if text.contains(word) {
            // Words are far stronger evidence than a single letter.
            p.persian_markers += 3;
        }
    }
    for word in ARABIC_WORDS {
        if text.contains(word) {
            p.arabic_markers += 3;
        }
    }

    p
}

/// Pick a language for a newly-joined group.
///
/// `fallback` is used when the text carries no usable signal at all (emoji-only
/// titles, digits, or an empty description).
pub fn detect_group_lang(title: &str, description: Option<&str>, fallback: Lang) -> Lang {
    let mut text = title.to_owned();
    if let Some(desc) = description {
        text.push(' ');
        text.push_str(desc);
    }

    let p = script_profile(&text);
    if p.total_letters() == 0 {
        return fallback;
    }

    let dominant = p.latin.max(p.cyrillic).max(p.arabic_script);

    if p.cyrillic == dominant {
        return Lang::Ru;
    }

    if p.arabic_script == dominant {
        return match p.persian_markers.cmp(&p.arabic_markers) {
            std::cmp::Ordering::Greater => Lang::Fa,
            std::cmp::Ordering::Less => Lang::Ar,
            // Ambiguous Perso-Arabic text defaults to Persian by design.
            std::cmp::Ordering::Equal => Lang::Fa,
        };
    }

    Lang::En
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyrillic_title_is_russian() {
        assert_eq!(detect_group_lang("Новости чат", None, Lang::En), Lang::Ru);
    }

    #[test]
    fn latin_title_is_english() {
        assert_eq!(
            detect_group_lang("Crypto Talk — Discussion", None, Lang::Fa),
            Lang::En
        );
    }

    #[test]
    fn persian_letters_beat_shared_script() {
        // «گفتگو» carries گ, which Arabic never uses.
        assert_eq!(detect_group_lang("گفتگوی آزاد", None, Lang::En), Lang::Fa);
    }

    #[test]
    fn arabic_letters_win_when_present() {
        assert_eq!(detect_group_lang("قناة الأخبار", None, Lang::En), Lang::Ar);
    }

    #[test]
    fn keywords_resolve_otherwise_ambiguous_text() {
        // No Persian-only letters here, but «مجموعة» is decisive.
        assert_eq!(
            detect_group_lang("مجموعة الأصدقاء", None, Lang::Fa),
            Lang::Ar
        );
    }

    #[test]
    fn ambiguous_perso_arabic_defaults_to_persian() {
        // "News" — spelled identically in both languages, no marker letters.
        assert_eq!(detect_group_lang("اخبار", None, Lang::En), Lang::Fa);
    }

    #[test]
    fn description_contributes_when_title_is_neutral() {
        assert_eq!(
            detect_group_lang("VIP 2024", Some("گروه تبادل نظر"), Lang::En),
            Lang::Fa
        );
    }

    #[test]
    fn emoji_only_title_uses_fallback() {
        assert_eq!(detect_group_lang("🔥🔥🔥 777", None, Lang::Ru), Lang::Ru);
    }

    #[test]
    fn mixed_script_follows_the_dominant_one() {
        assert_eq!(
            detect_group_lang("Tehran کانال گفتگو و تبادل نظر دوستان", None, Lang::En),
            Lang::Fa
        );
    }
}
