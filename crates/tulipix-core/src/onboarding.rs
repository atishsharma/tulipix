//! 5-step onboarding wizard state machine.
//!
//! Step order: Theme → Security → Mode → AI models → Libraries → Done.
//! Each step persists its answers into the in-flight `WizardState`;
//! `commit()` writes them into Settings + caps + AI model installer + library
//! roots. The wizard supports back/next nav, per-step skip (where allowed),
//! and resume across launches via `<config>/onboarding.json` so a half-
//! finished install picks up where the user left it.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step { Theme, Security, Mode, AiModels, Libraries, Done }

impl Step {
    pub const ORDER: [Step; 6] = [Step::Theme, Step::Security, Step::Mode, Step::AiModels, Step::Libraries, Step::Done];
    pub fn index(self) -> usize { Self::ORDER.iter().position(|s| *s == self).unwrap() }
    pub fn next(self) -> Option<Step> { Self::ORDER.get(self.index() + 1).copied() }
    pub fn prev(self) -> Option<Step> { if self.index() == 0 { None } else { Some(Self::ORDER[self.index() - 1]) } }
    pub fn is_skippable(self) -> bool { matches!(self, Step::AiModels | Step::Libraries) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice { System, Light, ExtraDark }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecurityChoice { None, Pin, Biometric, Passkey }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeChoice { Local, Account }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WizardState {
    pub current: Option<Step>,
    pub theme: Option<ThemeChoice>,
    pub security: Option<SecurityChoice>,
    pub mode: Option<ModeChoice>,
    pub ai_models_selected: Vec<String>,
    pub library_roots: Vec<PathBuf>,
    pub completed: bool,
}

impl WizardState {
    pub fn start() -> Self { Self { current: Some(Step::Theme), ..Default::default() } }

    pub fn ready(&self, step: Step) -> bool {
        match step {
            Step::Theme     => true,
            Step::Security  => self.theme.is_some(),
            Step::Mode      => self.theme.is_some() && self.security.is_some(),
            Step::AiModels  => self.theme.is_some() && self.security.is_some() && self.mode.is_some(),
            Step::Libraries => self.theme.is_some() && self.security.is_some() && self.mode.is_some(),
            Step::Done      => self.theme.is_some() && self.security.is_some() && self.mode.is_some(),
        }
    }

    pub fn advance(&mut self) -> Result<Step> {
        let cur = self.current.ok_or_else(|| anyhow!("wizard not started"))?;
        let Some(next) = cur.next() else { return Err(anyhow!("already at last step")); };
        if !self.ready(next) { return Err(anyhow!("step {:?} not ready", next)); }
        self.current = Some(next);
        if next == Step::Done { self.completed = true; }
        Ok(next)
    }

    pub fn back(&mut self) -> Result<Step> {
        let cur = self.current.ok_or_else(|| anyhow!("wizard not started"))?;
        let Some(prev) = cur.prev() else { return Err(anyhow!("already at first step")); };
        self.current = Some(prev);
        Ok(prev)
    }

    pub fn skip(&mut self) -> Result<Step> {
        let cur = self.current.ok_or_else(|| anyhow!("wizard not started"))?;
        if !cur.is_skippable() { return Err(anyhow!("step {:?} not skippable", cur)); }
        // Skipping preserves the per-step empty default — caller doesn't need to set it.
        let Some(next) = cur.next() else { return Err(anyhow!("already at last step")); };
        self.current = Some(next);
        if next == Step::Done { self.completed = true; }
        Ok(next)
    }

    pub fn progress(&self) -> f32 {
        let cur = self.current.unwrap_or(Step::Theme);
        // 0..1 across [Theme, Security, Mode, AiModels, Libraries, Done].
        cur.index() as f32 / (Step::ORDER.len() - 1) as f32
    }
}

pub fn state_path(config_dir: &Path) -> PathBuf { config_dir.join("onboarding.json") }

pub fn load(config_dir: &Path) -> Result<Option<WizardState>> {
    let p = state_path(config_dir);
    if !p.exists() { return Ok(None); }
    let bytes = std::fs::read(&p)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub fn save(config_dir: &Path, state: &WizardState) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let p = state_path(config_dir);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn clear(config_dir: &Path) -> Result<()> {
    let p = state_path(config_dir);
    if p.exists() { std::fs::remove_file(&p)?; }
    Ok(())
}

/// First-launch helper. Returns true when onboarding must run.
pub fn needs_onboarding(config_dir: &Path) -> bool {
    match load(config_dir) { Ok(Some(s)) => !s.completed, _ => !state_path(config_dir).with_file_name("settings.json").exists() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test] fn happy_path_walks_six_steps() {
        let mut s = WizardState::start();
        s.theme = Some(ThemeChoice::System);
        assert_eq!(s.advance().unwrap(), Step::Security);
        s.security = Some(SecurityChoice::Biometric);
        assert_eq!(s.advance().unwrap(), Step::Mode);
        s.mode = Some(ModeChoice::Local);
        assert_eq!(s.advance().unwrap(), Step::AiModels);
        assert_eq!(s.advance().unwrap(), Step::Libraries);
        assert_eq!(s.advance().unwrap(), Step::Done);
        assert!(s.completed);
    }
    #[test] fn cant_advance_when_step_not_ready() {
        let mut s = WizardState::start();
        assert!(s.advance().is_err());
    }
    #[test] fn skip_only_optional_steps() {
        let mut s = WizardState::start();
        s.theme = Some(ThemeChoice::Light);
        assert!(s.skip().is_err()); // Theme not skippable
        s.advance().unwrap();
        s.security = Some(SecurityChoice::None);
        s.advance().unwrap();
        s.mode = Some(ModeChoice::Local);
        s.advance().unwrap();
        assert_eq!(s.skip().unwrap(), Step::Libraries); // AiModels skipped
        assert_eq!(s.skip().unwrap(), Step::Done);      // Libraries skipped
        assert!(s.completed);
    }
    #[test] fn back_walks_backwards() {
        let mut s = WizardState::start();
        s.theme = Some(ThemeChoice::System);
        s.advance().unwrap();
        assert_eq!(s.back().unwrap(), Step::Theme);
        assert!(s.back().is_err());
    }
    #[test] fn load_save_round_trip() {
        let d = tempdir().unwrap();
        let mut s = WizardState::start();
        s.theme = Some(ThemeChoice::ExtraDark);
        s.library_roots.push(PathBuf::from("/tmp/photos"));
        save(d.path(), &s).unwrap();
        let loaded = load(d.path()).unwrap().unwrap();
        assert_eq!(loaded.theme, Some(ThemeChoice::ExtraDark));
        assert_eq!(loaded.library_roots, vec![PathBuf::from("/tmp/photos")]);
        clear(d.path()).unwrap();
        assert!(load(d.path()).unwrap().is_none());
    }
    #[test] fn progress_monotonically_increases() {
        let mut s = WizardState::start();
        let mut last = -1.0_f32;
        for _ in 0..Step::ORDER.len() - 1 {
            let p = s.progress();
            assert!(p > last); last = p;
            // Force-set the choice for whichever step we're on, then advance.
            match s.current.unwrap() {
                Step::Theme     => s.theme    = Some(ThemeChoice::System),
                Step::Security  => s.security = Some(SecurityChoice::None),
                Step::Mode      => s.mode     = Some(ModeChoice::Local),
                Step::AiModels | Step::Libraries | Step::Done => {}
            }
            if s.current != Some(Step::Done) { s.advance().unwrap(); }
        }
        assert_eq!(s.progress(), 1.0);
    }
}
