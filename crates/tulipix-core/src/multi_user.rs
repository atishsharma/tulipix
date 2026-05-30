//! Multiple local-mode users per OS account. Each profile lives under
//! `<config>/Tulipix-<slug>/` (and matching `<data>/` + `<cache>/`)
//! so family-laptop installs keep libraries, edits, and AI clusters
//! separate without OS user switching. The picker runs on launch when
//! more than one profile is present.

use crate::paths;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_SLUG: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub slug: String,
    pub display_name: String,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { DEFAULT_SLUG.into() } else { trimmed }
}

fn suffix_for(slug: &str) -> String {
    if slug == DEFAULT_SLUG { String::new() } else { format!("-{slug}") }
}

pub fn profile_paths(slug: &str) -> Option<Profile> {
    let cfg = paths::config_dir()?;
    let dat = paths::data_dir()?;
    let cac = paths::cache_dir()?;
    let s = suffix_for(slug);
    let parent_cfg = cfg.parent()?;
    let parent_dat = dat.parent()?;
    let parent_cac = cac.parent()?;
    Some(Profile {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        config_dir: parent_cfg.join(format!("Tulipix{s}")),
        data_dir:   parent_dat.join(format!("Tulipix{s}")),
        cache_dir:  parent_cac.join(format!("Tulipix{s}")),
    })
}

pub fn list_profiles_under(parent_config: &std::path::Path) -> Result<Vec<String>> {
    let mut slugs: Vec<String> = Vec::new();
    if !parent_config.exists() { return Ok(slugs); }
    for ent in std::fs::read_dir(parent_config)? {
        let ent = ent?;
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == "Tulipix" { slugs.push(DEFAULT_SLUG.into()); continue; }
        if let Some(slug) = name.strip_prefix("Tulipix-") {
            if !slug.is_empty() { slugs.push(slug.to_string()); }
        }
    }
    slugs.sort();
    Ok(slugs)
}

pub fn create_profile(parent_config: &std::path::Path, display_name: &str) -> Result<String> {
    let slug = slugify(display_name);
    let dir = parent_config.join(format!("Tulipix{}", suffix_for(&slug)));
    if dir.exists() { return Err(anyhow!("profile {slug:?} already exists")); }
    std::fs::create_dir_all(&dir)?;
    Ok(slug)
}

pub fn needs_picker(parent_config: &std::path::Path) -> bool {
    list_profiles_under(parent_config).map(|p| p.len() > 1).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test] fn slug_handles_spaces_and_unicode() {
        assert_eq!(slugify("Family Mom"), "family-mom");
        assert_eq!(slugify("  ___ "), DEFAULT_SLUG);
        assert_eq!(slugify("Mom's Photos!"), "mom-s-photos");
    }
    #[test] fn list_finds_default_and_named() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Tulipix")).unwrap();
        std::fs::create_dir_all(d.path().join("Tulipix-kid")).unwrap();
        std::fs::create_dir_all(d.path().join("Tulipix-mom")).unwrap();
        std::fs::create_dir_all(d.path().join("Other")).unwrap();
        let slugs = list_profiles_under(d.path()).unwrap();
        assert_eq!(slugs, vec!["default", "kid", "mom"]);
    }
    #[test] fn create_then_listed() {
        let d = tempdir().unwrap();
        let slug = create_profile(d.path(), "Dad's Library").unwrap();
        assert_eq!(slug, "dad-s-library");
        assert!(list_profiles_under(d.path()).unwrap().contains(&slug));
        // re-create same name fails
        assert!(create_profile(d.path(), "Dad's Library").is_err());
    }
    #[test] fn picker_needed_when_more_than_one() {
        let d = tempdir().unwrap();
        assert!(!needs_picker(d.path()));
        std::fs::create_dir_all(d.path().join("Tulipix")).unwrap();
        assert!(!needs_picker(d.path()));
        std::fs::create_dir_all(d.path().join("Tulipix-mom")).unwrap();
        assert!(needs_picker(d.path()));
    }
}
