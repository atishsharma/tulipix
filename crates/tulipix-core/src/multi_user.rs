//! Profiles: several people sharing one computer account, each with their own
//! library, settings and lock-screen PIN.
//!
//! Not OS accounts. The setting used to say "separate library per computer
//! user", which the OS already does by giving each account its own home
//! directory — a switch for it could only ever have been a switch for
//! nothing. What people actually have is one laptop, one login, and a
//! household: this is that.
//!
//! A profile is a folder suffix. `TULIPIX_PROFILE=mom` makes every path
//! `Tulipix-mom` instead of `Tulipix` (see `paths::profile_suffix`), so a
//! profile is a whole parallel library with nothing shared — not a filtered
//! view of one. Switching means restarting with that variable set, which is
//! the only honest way: a running process has its databases open.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths;

/// The profile everybody starts with, whose folders have no suffix.
pub const DEFAULT_SLUG: &str = "default";
/// The Settings switch. Off, there is one library and no picker.
pub const FLAG: &str = "multi-user";
/// Where a profile's display name is kept, inside its own config folder.
const NAME_FILE: &str = "profile-name.txt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The folder suffix, and the value of `TULIPIX_PROFILE`.
    pub slug: String,
    /// What the picker shows. The slug when nothing was typed.
    pub display_name: String,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Whether this is the profile the running app is using.
    pub active: bool,
}

pub fn slugify(name: &str) -> String {
    let slug = paths::slugify_profile(name);
    if slug.is_empty() { DEFAULT_SLUG.into() } else { slug }
}

fn suffix_for(slug: &str) -> String {
    if slug == DEFAULT_SLUG { String::new() } else { format!("-{slug}") }
}

/// Which profile this process is running as.
pub fn active_slug() -> String {
    let suffix = paths::profile_suffix();
    match suffix.strip_prefix('-') {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => DEFAULT_SLUG.to_string(),
    }
}

/// The three folders a profile would use, whether or not they exist.
pub fn profile_paths(slug: &str) -> Option<Profile> {
    let s = suffix_for(slug);
    let parent = |d: Option<PathBuf>| -> Option<PathBuf> {
        d.and_then(|d| d.parent().map(Path::to_path_buf))
    };
    let (cfg, dat, cac) = (
        parent(paths::config_dir())?,
        parent(paths::data_dir())?,
        parent(paths::cache_dir())?,
    );
    Some(Profile {
        display_name: read_name(&cfg.join(format!("Tulipix{s}"))).unwrap_or_else(|| slug.to_string()),
        config_dir: cfg.join(format!("Tulipix{s}")),
        data_dir: dat.join(format!("Tulipix{s}")),
        cache_dir: cac.join(format!("Tulipix{s}")),
        active: slug == active_slug(),
        slug: slug.to_string(),
    })
}

fn read_name(config_dir: &Path) -> Option<String> {
    let name = std::fs::read_to_string(config_dir.join(NAME_FILE)).ok()?;
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub fn set_display_name(slug: &str, name: &str) -> Result<()> {
    let p = profile_paths(slug).ok_or_else(|| anyhow!("no home directory on this system"))?;
    std::fs::create_dir_all(&p.config_dir)?;
    std::fs::write(p.config_dir.join(NAME_FILE), name.trim())?;
    Ok(())
}

/// Every profile on this computer, the default one first and the rest by name.
/// Read from the folders themselves, so a profile made by hand shows up and
/// one deleted by hand disappears.
pub fn list() -> Vec<Profile> {
    let Some(parent) = paths::config_dir().and_then(|d| d.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    let mut slugs = list_slugs_under(&parent).unwrap_or_default();
    // The one this process is running as always exists, even before anything
    // has been written into it.
    let active = active_slug();
    if !slugs.contains(&active) {
        slugs.push(active);
    }
    if !slugs.contains(&DEFAULT_SLUG.to_string()) {
        slugs.push(DEFAULT_SLUG.to_string());
    }
    slugs.sort_by(|a, b| match (a.as_str(), b.as_str()) {
        (DEFAULT_SLUG, DEFAULT_SLUG) => std::cmp::Ordering::Equal,
        (DEFAULT_SLUG, _) => std::cmp::Ordering::Less,
        (_, DEFAULT_SLUG) => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    });
    slugs.dedup();
    slugs.iter().filter_map(|s| profile_paths(s)).collect()
}

pub fn list_slugs_under(parent_config: &Path) -> Result<Vec<String>> {
    let mut slugs: Vec<String> = Vec::new();
    if !parent_config.exists() {
        return Ok(slugs);
    }
    for ent in std::fs::read_dir(parent_config)? {
        let ent = ent?;
        if !ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == "Tulipix" {
            slugs.push(DEFAULT_SLUG.into());
            continue;
        }
        if let Some(slug) = name.strip_prefix("Tulipix-") {
            if !slug.is_empty() {
                slugs.push(slug.to_string());
            }
        }
    }
    slugs.sort();
    Ok(slugs)
}

/// Make a profile. Its folders come into being here so the picker can show it
/// before it has ever been opened.
pub fn create(display_name: &str) -> Result<String> {
    let slug = slugify(display_name);
    if slug == DEFAULT_SLUG {
        return Err(anyhow!("that name is reserved — pick another"));
    }
    let p = profile_paths(&slug).ok_or_else(|| anyhow!("no home directory on this system"))?;
    if p.config_dir.exists() {
        return Err(anyhow!("there is already a profile called {display_name}"));
    }
    std::fs::create_dir_all(&p.config_dir)?;
    std::fs::create_dir_all(&p.data_dir)?;
    std::fs::create_dir_all(&p.cache_dir)?;
    set_display_name(&slug, display_name)?;
    Ok(slug)
}

/// Delete a profile and everything in it. Never the one in use, and never the
/// default: the default is where a first-run library lives, and deleting it
/// would be a factory reset wearing a different name.
pub fn delete(slug: &str) -> Result<()> {
    if slug == DEFAULT_SLUG {
        return Err(anyhow!("the first profile cannot be deleted"));
    }
    if slug == active_slug() {
        return Err(anyhow!("switch to another profile before deleting this one"));
    }
    let p = profile_paths(slug).ok_or_else(|| anyhow!("no home directory on this system"))?;
    if !p.config_dir.exists() {
        return Err(anyhow!("there is no profile called {slug}"));
    }
    for dir in [&p.config_dir, &p.data_dir, &p.cache_dir] {
        std::fs::remove_dir_all(dir).ok();
    }
    Ok(())
}

/// Whether the launcher should offer a choice: the switch is on and there is
/// more than one profile to choose between.
pub fn needs_picker() -> bool {
    crate::settings::Settings::load()
        .map(|s| s.flag(FLAG, false))
        .unwrap_or(false)
        && list().len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_name_becomes_a_folder_safe_slug() {
        assert_eq!(slugify("Family Mom"), "family-mom");
        assert_eq!(slugify("Mom's Photos!"), "mom-s-photos");
        // Nothing usable falls back to the default, which `create` then
        // refuses — a profile may not be called that.
        assert_eq!(slugify("  ___ "), DEFAULT_SLUG);
        // A name that would escape the folder cannot.
        assert_eq!(slugify("../../etc"), "etc");
        assert!(!slugify("a/b").contains('/'));
    }

    #[test]
    fn folders_are_read_back_as_profiles_and_other_folders_are_not() {
        let d = tempdir().unwrap();
        for name in ["Tulipix", "Tulipix-kid", "Tulipix-mom", "Other", "Tulipix-"] {
            std::fs::create_dir_all(d.path().join(name)).unwrap();
        }
        std::fs::write(d.path().join("Tulipix-file"), b"not a directory").ok();
        let slugs = list_slugs_under(d.path()).unwrap();
        assert_eq!(slugs, vec!["default", "kid", "mom"]);
    }

    #[test]
    fn an_empty_parent_has_no_profiles_and_a_missing_one_is_not_an_error() {
        let d = tempdir().unwrap();
        assert!(list_slugs_under(d.path()).unwrap().is_empty());
        assert!(list_slugs_under(&d.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn the_default_profile_is_protected() {
        assert!(delete(DEFAULT_SLUG).is_err());
        // And a name that slugs to it is refused at creation.
        assert!(create("default").is_err());
        assert!(create("  ").is_err());
    }

    #[test]
    fn the_active_slug_comes_from_the_environment_or_is_the_default() {
        // `profile_suffix` is read once per process, so this checks the
        // mapping rather than re-reading the variable.
        assert_eq!(
            match "-mom".strip_prefix('-') {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => DEFAULT_SLUG.to_string(),
            },
            "mom"
        );
        assert_eq!(active_slug(), DEFAULT_SLUG, "tests run without TULIPIX_PROFILE");
    }
}
