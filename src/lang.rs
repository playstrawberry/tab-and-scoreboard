//! The plugin's texts in English, French, Spanish and Turkish (`lang/<code>.lang`, `key=text`
//! lines; `{0}`, `{1}`, `{2}` are filled in). Any other language reads English.
//!
//! The `default.*` keys are the built-in layout: a settings value `@default.header` shows that
//! text in each viewer's language.
//!
//! A server owner may add `plugins/data/tab-and-scoreboard/lang/<code>.lang` files ([`OwnerLang`])
//! for texts of their own (`@my.key` in the settings), and may replace any built-in text there by
//! using its key.

use std::collections::HashMap;

pub const FILES: [(&str, &str); 4] = [
    ("en", include_str!("../lang/en.lang")),
    ("fr", include_str!("../lang/fr.lang")),
    ("es", include_str!("../lang/es.lang")),
    ("tr", include_str!("../lang/tr.lang")),
];

/// The keys every language file has.
pub const KEYS: [&str; 26] = [
    "default.header",
    "default.footer",
    "default.title",
    "default.line.rank",
    "default.line.world",
    "default.line.online",
    "default.line.ping",
    "status",
    "status.no_perms",
    "reloaded",
    "kept",
    "usage",
    "err.read",
    "err.too_big",
    "err.syntax",
    "err.unknown",
    "err.duplicate",
    "err.value",
    "err.limit",
    "err.font",
    "err.icon",
    "err.lang",
    "sidebar.on",
    "sidebar.off",
    "sidebar.state",
    "sidebar.usage",
];

/// The largest owner language file read.
pub const MAX_OWNER_FILE_BYTES: usize = 64 * 1024;

/// The index in [`FILES`] of the language of a locale written as Minecraft writes it
/// (`fr_fr`, `FrCa`, `tr-tr`); 0 (English) for any other.
#[must_use]
pub fn index(locale: &str) -> usize {
    let code = locale.get(..2).unwrap_or("").to_ascii_lowercase();
    FILES.iter().position(|(c, _)| *c == code).unwrap_or(0)
}

fn lookup<'a>(file: &'a str, key: &str) -> Option<&'a str> {
    file.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

fn fill(raw: &str, args: &[&str]) -> String {
    let mut out = raw.to_owned();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), arg);
    }
    out
}

/// The text of `key` in language `lang` (an index from [`index`]), with `args` in place of
/// `{0}`, `{1}`, ...; English when the language lacks the key, the key itself when English
/// lacks it too.
#[must_use]
pub fn text(lang: usize, key: &str, args: &[&str]) -> String {
    let raw = FILES
        .get(lang)
        .and_then(|(_, f)| lookup(f, key))
        .or_else(|| lookup(FILES[0].1, key))
        .unwrap_or(key);
    fill(raw, args)
}

/// A server owner's language files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerLang {
    files: [HashMap<String, String>; 4],
}

impl OwnerLang {
    /// Reads the file of language `lang`.
    ///
    /// # Errors
    ///
    /// The line (1-based) that is not `key=text` with a key the settings could name.
    pub fn read(&mut self, lang: usize, text: &str) -> Result<(), usize> {
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(index + 1);
            };
            let key = key.trim();
            if !crate::placeholders::valid_key(key) {
                return Err(index + 1);
            }
            self.files[lang.min(3)].insert(key.to_owned(), value.trim().to_owned());
        }
        Ok(())
    }

    /// Whether any language has `key`, here or among the built-in texts.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.files.iter().any(|f| f.contains_key(key)) || lookup(FILES[0].1, key).is_some()
    }

    /// The text of `key` in language `lang`: the owner's in that language, the built-in one in
    /// that language, the owner's English, the built-in English, the key itself; with `args`.
    #[must_use]
    pub fn text(&self, lang: usize, key: &str, args: &[&str]) -> String {
        let lang = lang.min(3);
        let raw = self.files[lang]
            .get(key)
            .map(String::as_str)
            .or_else(|| lookup(FILES[lang].1, key))
            .or_else(|| self.files[0].get(key).map(String::as_str))
            .or_else(|| lookup(FILES[0].1, key))
            .unwrap_or(key);
        fill(raw, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_key_with_the_same_placeholders() {
        for (code, file) in FILES {
            for key in KEYS {
                let found = lookup(file, key).unwrap_or_else(|| panic!("{code} lacks {key}"));
                let english = lookup(FILES[0].1, key).unwrap();
                for slot in [
                    "{0}",
                    "{1}",
                    "{2}",
                    "{player}",
                    "{ping}",
                    "{online}",
                    "{max_players}",
                    "{world}",
                    "{group}",
                    "{tps}",
                ] {
                    assert_eq!(
                        found.contains(slot),
                        english.contains(slot),
                        "{code} {key} {slot}"
                    );
                }
            }
            let keys = file.lines().filter(|l| l.contains('=')).count();
            assert_eq!(keys, KEYS.len(), "{code} has keys no code reads");
        }
    }

    #[test]
    fn locales_pick_their_language_and_others_read_english() {
        assert_eq!(index("fr_fr"), 1);
        assert_eq!(index("FrCa"), 1);
        assert_eq!(index("es_mx"), 2);
        assert_eq!(index("tr-tr"), 3);
        assert_eq!(index("de_de"), 0);
        assert_eq!(index(""), 0);
        assert_ne!(text(3, "reloaded", &["2"]), text(0, "reloaded", &["2"]));
        assert!(text(0, "reloaded", &["2"]).contains('2'));
        assert_eq!(text(0, "no_such_key", &[]), "no_such_key");
    }

    #[test]
    fn an_owners_text_wins_in_its_own_language_and_falls_back_in_a_sensible_order() {
        let mut owner = OwnerLang::default();
        owner
            .read(3, "# a note\n\nsidebar.on=Kurulu {0}\nmy.title=Merhaba\n")
            .expect("reads");
        owner.read(0, "my.title=Hello\n").expect("reads");
        // The owner's Turkish; another language keeps its built-in text.
        assert_eq!(owner.text(3, "sidebar.on", &["x"]), "Kurulu x");
        assert_eq!(owner.text(0, "sidebar.on", &[]), text(0, "sidebar.on", &[]));
        // A key only the owner has: the owner's English for the languages without it.
        assert_eq!(owner.text(1, "my.title", &[]), "Hello");
        assert_eq!(owner.text(3, "my.title", &[]), "Merhaba");
        assert!(owner.has("my.title"));
        assert!(owner.has("default.header"));
        assert!(!owner.has("my.none"));
        assert_eq!(owner.text(0, "my.none", &[]), "my.none");
    }

    #[test]
    fn a_bad_owner_line_is_named() {
        let mut owner = OwnerLang::default();
        assert_eq!(owner.read(0, "ok=1\nno equals\n"), Err(2));
        assert_eq!(owner.read(0, "Bad Key=1\n"), Err(1));
    }
}
