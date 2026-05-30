//! Minimal Fluent-style localisation. Parses `key = value` lines from
//! ui/locales/{lang}.ftl files, supports `{ $name }` positional
//! substitution. CI gate flags any key present in the English baseline
//! but missing from a translation file.
//!
//! Initial locale set: en/es/fr/de/ja/zh-CN/hi/pt-BR. The Slint side
//! marks strings with @tr(); the binding generator emits the key, the
//! runtime resolves via Bundle::format.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Locale {
    #[default] En,
    Es, Fr, De, Ja, ZhCn, Hi, PtBr,
}

impl Locale {
    pub fn code(self) -> &'static str {
        match self {
            Self::En   => "en",
            Self::Es   => "es",
            Self::Fr   => "fr",
            Self::De   => "de",
            Self::Ja   => "ja",
            Self::ZhCn => "zh-CN",
            Self::Hi   => "hi",
            Self::PtBr => "pt-BR",
        }
    }
    pub fn all() -> &'static [Locale] {
        &[Self::En, Self::Es, Self::Fr, Self::De, Self::Ja, Self::ZhCn, Self::Hi, Self::PtBr]
    }
    pub fn parse(code: &str) -> Option<Self> {
        Self::all().iter().copied().find(|l| l.code().eq_ignore_ascii_case(code))
    }
}

#[derive(Debug, Default, Clone)]
pub struct Bundle {
    pub locale: Option<Locale>,
    pub messages: BTreeMap<String, String>,
}

impl Bundle {
    pub fn from_ftl(locale: Locale, src: &str) -> Result<Self> {
        let mut messages = BTreeMap::new();
        for (lineno, raw) in src.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else {
                return Err(anyhow!("ftl line {}: missing '='", lineno + 1));
            };
            let key = k.trim();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
                return Err(anyhow!("ftl line {}: invalid key {key:?}", lineno + 1));
            }
            messages.insert(key.to_string(), v.trim().to_string());
        }
        Ok(Self { locale: Some(locale), messages })
    }

    pub fn load(locales_dir: &Path, locale: Locale) -> Result<Self> {
        let path = locales_dir.join(format!("{}.ftl", locale.code()));
        let src = std::fs::read_to_string(&path)?;
        Self::from_ftl(locale, &src)
    }

    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> Option<String> {
        let template = self.messages.get(key)?;
        let mut out = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                let mut placeholder = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' { closed = true; break; }
                    placeholder.push(inner);
                }
                if !closed { out.push('{'); out.push_str(&placeholder); continue; }
                let trimmed = placeholder.trim();
                let name = trimmed.strip_prefix('$').unwrap_or(trimmed);
                if let Some((_, v)) = args.iter().find(|(k, _)| *k == name) {
                    out.push_str(v);
                } else {
                    out.push('{'); out.push_str(&placeholder); out.push('}');
                }
            } else {
                out.push(c);
            }
        }
        Some(out)
    }
}

/// CI gate output. Empty `missing` means the translation matches the baseline.
#[derive(Debug, Default, Clone)]
pub struct MissingKeysReport {
    pub locale: Locale,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

impl MissingKeysReport { pub fn ok(&self) -> bool { self.missing.is_empty() } }

pub fn diff_against_baseline(baseline: &Bundle, other: &Bundle) -> MissingKeysReport {
    let mut missing: Vec<String> = baseline.messages.keys().filter(|k| !other.messages.contains_key(*k)).cloned().collect();
    let mut extra: Vec<String> = other.messages.keys().filter(|k| !baseline.messages.contains_key(*k)).cloned().collect();
    missing.sort(); extra.sort();
    MissingKeysReport { locale: other.locale.unwrap_or(Locale::En), missing, extra }
}

/// CI helper — scans a locales directory, returns a report per non-baseline locale.
pub fn audit_locales(locales_dir: &Path) -> Result<Vec<MissingKeysReport>> {
    let baseline = Bundle::load(locales_dir, Locale::En)?;
    let mut out = Vec::new();
    for &locale in Locale::all() {
        if locale == Locale::En { continue; }
        let path: PathBuf = locales_dir.join(format!("{}.ftl", locale.code()));
        if !path.exists() {
            out.push(MissingKeysReport { locale, missing: baseline.messages.keys().cloned().collect(), extra: vec![] });
            continue;
        }
        let other = Bundle::load(locales_dir, locale)?;
        out.push(diff_against_baseline(&baseline, &other));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test] fn parse_simple_and_format() {
        let b = Bundle::from_ftl(Locale::En, "# top comment\nhello = Hello, { $name }!\nbye=Goodbye\n").unwrap();
        assert_eq!(b.format("hello", &[("name", "Tulip")]).unwrap(), "Hello, Tulip!");
        assert_eq!(b.format("bye", &[]).unwrap(), "Goodbye");
        assert!(b.format("missing", &[]).is_none());
    }
    #[test] fn rejects_bad_key() {
        assert!(Bundle::from_ftl(Locale::En, "bad key = x\n").is_err());
        assert!(Bundle::from_ftl(Locale::En, "noeq\n").is_err());
    }
    #[test] fn locale_round_trip() {
        for &l in Locale::all() { assert_eq!(Locale::parse(l.code()), Some(l)); }
        assert_eq!(Locale::parse("ZH-cn"), Some(Locale::ZhCn));
        assert!(Locale::parse("xx").is_none());
    }
    #[test] fn audit_detects_missing_and_extra() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("en.ftl"), "a = A\nb = B\n").unwrap();
        std::fs::write(dir.path().join("fr.ftl"), "a = Aa\nc = Cc\n").unwrap();
        let reports = audit_locales(dir.path()).unwrap();
        let fr = reports.iter().find(|r| r.locale == Locale::Fr).unwrap();
        assert_eq!(fr.missing, vec!["b".to_string()]);
        assert_eq!(fr.extra, vec!["c".to_string()]);
        assert!(!fr.ok());
        // Locale with no file present at all reports every baseline key as missing.
        let de = reports.iter().find(|r| r.locale == Locale::De).unwrap();
        assert_eq!(de.missing.len(), 2);
    }
    #[test] fn placeholder_unclosed_is_literal() {
        let b = Bundle::from_ftl(Locale::En, "x = oops { $foo").unwrap();
        assert_eq!(b.format("x", &[("foo", "ignored")]).unwrap(), "oops { $foo");
    }
}
