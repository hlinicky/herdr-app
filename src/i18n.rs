//! The interface's language: settings.json's `language`, else the system's.
//!
//! Texts are written in English where they're used; a language other than
//! English is a catalog (`defaults/i18n/<code>.json`) from the English text
//! to its translation. A text missing from the catalog shows in English, so
//! a new or changed text never shows empty. `{name}` in a text is filled from
//! the values passed with it. A translation may be a list of plural forms,
//! picked by the value `n` (Slovak: 1, 2–4, 5 and more).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// What settings.json's `language` may say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// The system's language when Herdr has it, else English.
    #[default]
    System,
    En,
    Sk,
}

impl Language {
    pub fn is_system(&self) -> bool {
        *self == Language::System
    }

    /// The language shown: a code from [`LANGUAGES`].
    pub fn resolve(self) -> &'static str {
        match self {
            Language::System => system(),
            Language::En => "en",
            Language::Sk => "sk",
        }
    }
}

/// Every language Herdr speaks: its code and its own name for itself.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("sk", "Slovenčina")];

const CATALOGS: &[(&str, &str)] = &[("sk", include_str!("../defaults/i18n/sk.json"))];

fn catalog(lang: &str) -> Option<&'static HashMap<String, Value>> {
    static PARSED: OnceLock<HashMap<&'static str, HashMap<String, Value>>> = OnceLock::new();
    PARSED
        .get_or_init(|| {
            CATALOGS
                .iter()
                .map(|(code, text)| {
                    let map: HashMap<String, Value> = serde_json::from_str(text).unwrap_or_else(|e| {
                        tracing::warn!("defaults/i18n/{code}.json: {e}");
                        HashMap::new()
                    });
                    (*code, map)
                })
                .collect()
        })
        .get(lang)
}

/// Which plural form of a list fits `n` in `lang`.
fn plural(lang: &str, n: f64, forms: usize) -> usize {
    let i = match lang {
        // Slovak: 1 kus, 2–4 kusy, 0 and 5+ kusov (and fractions).
        "sk" if n == 1.0 => 0,
        "sk" if n.fract() == 0.0 && (2.0..=4.0).contains(&n) => 1,
        "sk" => 2,
        _ if n == 1.0 => 0,
        _ => 1,
    };
    i.min(forms.saturating_sub(1))
}

/// `text` in `lang`, with `{name}` filled from `vars` (an object, or null).
pub fn translate(lang: &str, text: &str, vars: &Value) -> String {
    let n = vars.get("n").and_then(Value::as_f64);
    let found = catalog(lang).and_then(|c| c.get(text)).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.as_str()),
        Value::Array(forms) if !forms.is_empty() => forms[plural(lang, n.unwrap_or(0.0), forms.len())].as_str(),
        _ => None,
    });
    fill(found.unwrap_or(text), vars)
}

/// `text` in the language shown now.
pub fn t(text: &str) -> String {
    translate(&crate::settings::get().language, text, &Value::Null)
}

/// `text` in the language shown now, with `{name}` filled.
pub fn tv(text: &str, vars: Value) -> String {
    translate(&crate::settings::get().language, text, &vars)
}

fn fill(text: &str, vars: &Value) -> String {
    let Some(vars) = vars.as_object().filter(|o| !o.is_empty()) else { return text.to_owned() };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}').map(|close| (&after[..close], close)) {
            Some((name, close)) if vars.contains_key(name) => {
                match &vars[name] {
                    Value::String(s) => out.push_str(s),
                    Value::Null => {}
                    v => out.push_str(&v.to_string()),
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The system's language, if Herdr speaks it (else English). Read once.
fn system() -> &'static str {
    static FOUND: OnceLock<&'static str> = OnceLock::new();
    FOUND.get_or_init(|| system_locale().and_then(|l| known(&l)).unwrap_or("en"))
}

/// `sk_SK.UTF-8`, `sk-SK` or `sk` → `sk`, when Herdr speaks it.
fn known(locale: &str) -> Option<&'static str> {
    let code = locale.split(['_', '-', '.', '@']).next()?.to_ascii_lowercase();
    LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == code)
}

fn system_locale() -> Option<String> {
    // The POSIX order of precedence; a GUI app on macOS or Windows usually has none.
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
            && v != "C"
            && v != "POSIX"
            && !v.starts_with("C.")
        {
            return Some(v);
        }
    }
    os_locale()
}

/// Windows: the display language the person chose (not the region's formats).
#[cfg(windows)]
fn os_locale() -> Option<String> {
    // kernel32 is always linked; declared here to keep windows-sys' features as they are.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: takes no arguments and only reads the user's setting.
    let id = unsafe { GetUserDefaultUILanguage() };
    // The primary language is the low 10 bits of a LANGID; 0x1b is Slovak.
    Some(match id & 0x3ff {
        0x1b => "sk".into(),
        _ => "en".into(),
    })
}

/// macOS: the first of the person's preferred languages.
#[cfg(target_os = "macos")]
fn os_locale() -> Option<String> {
    let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split('"').nth(1).map(str::to_owned).or_else(|| text.lines().nth(1).map(|l| l.trim().trim_end_matches(',').to_owned()))
}

/// Linux has only the environment, read above.
#[cfg(not(any(windows, target_os = "macos")))]
fn os_locale() -> Option<String> {
    std::env::var("LANGUAGE").ok().and_then(|v| v.split(':').next().map(str::to_owned)).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn english_is_the_text_itself() {
        assert_eq!(translate("en", "Settings", &Value::Null), "Settings");
        assert_eq!(translate("en", "Renamed {from} to {to}.", &json!({"from": "a", "to": "b"})), "Renamed a to b.");
    }

    #[test]
    fn missing_translations_fall_back_to_english() {
        assert_eq!(translate("sk", "A text nobody translated yet", &Value::Null), "A text nobody translated yet");
        assert_eq!(translate("xx", "Settings", &Value::Null), "Settings");
    }

    #[test]
    fn slovak_translates_and_fills_values() {
        assert_eq!(translate("sk", "Settings", &Value::Null), "Nastavenia");
        assert_eq!(translate("sk", "Language", &Value::Null), "Jazyk");
    }

    #[test]
    fn unknown_or_unfilled_braces_stay() {
        assert_eq!(fill("{a} and {b} {", &json!({"a": 1})), "1 and {b} {");
        assert_eq!(fill("{a}", &Value::Null), "{a}");
    }

    #[test]
    fn slovak_plurals() {
        let forms = |n: f64| plural("sk", n, 3);
        assert_eq!([forms(1.0), forms(2.0), forms(4.0), forms(5.0), forms(0.0), forms(1.5)], [0, 1, 1, 2, 2, 2]);
        assert_eq!(plural("en", 1.0, 2), 0);
        assert_eq!(plural("en", 3.0, 2), 1);
        assert_eq!(plural("sk", 3.0, 1), 0, "a single form fits every count");
    }

    #[test]
    fn locales_map_to_languages_herdr_speaks() {
        assert_eq!(known("sk_SK.UTF-8"), Some("sk"));
        assert_eq!(known("sk-SK"), Some("sk"));
        assert_eq!(known("SK"), Some("sk"));
        assert_eq!(known("en_US"), Some("en"));
        assert_eq!(known("de_DE"), None);
    }

    #[test]
    fn every_language_has_a_catalog_or_is_english() {
        for (code, _) in LANGUAGES {
            assert!(*code == "en" || catalog(code).is_some_and(|c| !c.is_empty()), "no catalog for {code}");
        }
    }

    /// A JS string literal's text (the escapes shipped mods use).
    fn unescape_js(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                }
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    /// The literal texts passed to `call"…"` (or `call'…'`) in `text`; `call` ends with its quote.
    fn literals(text: &str, call: &str) -> Vec<String> {
        let quote = call.chars().next_back().unwrap();
        let mut found = vec![];
        for (at, _) in text.match_indices(call) {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') && !call.starts_with("i18n") {
                continue;
            }
            let rest = &text[at + call.len()..];
            let mut end = None;
            let mut escaped = false;
            for (i, c) in rest.char_indices() {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    c if c == quote => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(end) = end {
                found.push(unescape_js(&rest[..end]));
            }
        }
        found
    }

    fn files(dir: &std::path::Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files(&path, ext, out);
            } else if path.extension().is_some_and(|e| e == ext) {
                out.push(path);
            }
        }
    }

    /// Every text the shipped mods and the app pass literally has a Slovak
    /// translation (Herdr 98 speaks only English, as its original did).
    #[test]
    fn every_shipped_text_has_a_slovak_translation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut paths = vec![];
        files(&root.join("defaults/mods"), "js", &mut paths);
        files(&root.join("defaults/frame"), "js", &mut paths);
        paths.retain(|p| !p.components().any(|c| c.as_os_str() == "win98"));
        let mut texts: Vec<(String, String)> = vec![];
        for path in &paths {
            let text = std::fs::read_to_string(path).unwrap();
            for call in ["tr(\"", "tr('"] {
                texts.extend(literals(&text, call).into_iter().map(|t| (path.display().to_string(), t)));
            }
        }
        let mut rust = vec![];
        files(&root.join("src"), "rs", &mut rust);
        for path in rust.iter().filter(|p| !p.ends_with("i18n.rs")) {
            let text = std::fs::read_to_string(path).unwrap();
            for call in ["i18n::t(\"", "i18n::tv(\""] {
                texts.extend(literals(&text, call).into_iter().map(|t| (path.display().to_string(), t)));
            }
        }
        assert!(texts.len() > 100, "found too few texts: the scan is broken");
        let sk = catalog("sk").unwrap();
        let mut missing: Vec<String> = texts.iter().filter(|(_, t)| !sk.contains_key(t)).map(|(f, t)| format!("{f}: {t:?}")).collect();
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "defaults/i18n/sk.json lacks:\n{}", missing.join("\n"));
    }

    /// Every translation keeps the `{name}` values of its English text.
    #[test]
    fn translations_keep_their_placeholders() {
        let names = |s: &str| {
            let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_owned())).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_')).collect();
            v.sort();
            v.dedup();
            v
        };
        let mut bad = vec![];
        for (code, _) in CATALOGS {
            for (english, value) in catalog(code).unwrap() {
                let forms: Vec<&str> = match value {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                    _ => vec![],
                };
                for form in forms {
                    // A plural form may leave out the count ("one tab").
                    let want: Vec<String> = names(english).into_iter().filter(|n| !(n == "n" && value.is_array())).collect();
                    let have = names(form);
                    if want.iter().any(|n| !have.contains(n)) || have.iter().any(|n| !names(english).contains(n)) {
                        bad.push(format!("{code}: {english:?} → {form:?}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "placeholders differ:\n{}", bad.join("\n"));
    }
}
