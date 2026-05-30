//! `np.p4.cloud.ransomware` — mass-rename/extension-change heuristic → freeze
//! + alert.
//!
//! Ransomware mass-renames files (often appending an extension like `.locked`)
//! in a short window. Before a sync pushes deletes/renames upstream, this scores
//! the change batch; a high score freezes sync and alerts the user instead of
//! propagating the damage.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeBatch {
    /// Files renamed in this batch (old_ext, new_ext).
    pub renames: Vec<(String, String)>,
    pub deletes: usize,
    pub total_files: usize,
}

/// Suspicion score 0..1. Driven by: fraction of files touched, fraction of
/// renames that add a single new uniform extension, and bulk deletes.
pub fn suspicion(batch: &ChangeBatch) -> f64 {
    if batch.total_files == 0 { return 0.0; }
    let touched = (batch.renames.len() + batch.deletes) as f64 / batch.total_files as f64;

    // Do most renames append the SAME new extension? (classic ransomware tell.)
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for (old, new) in &batch.renames {
        if new != old { *counts.entry(new.clone()).or_default() += 1; }
    }
    let uniform = counts.values().copied().max().unwrap_or(0) as f64;
    let uniform_frac = if batch.renames.is_empty() { 0.0 } else { uniform / batch.renames.len() as f64 };

    (0.6 * touched + 0.4 * uniform_frac).clamp(0.0, 1.0)
}

pub const FREEZE_THRESHOLD: f64 = 0.5;

/// Should sync freeze for this batch?
pub fn should_freeze(batch: &ChangeBatch) -> bool {
    suspicion(batch) >= FREEZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mass_uniform_rename_freezes() {
        let renames = (0..90).map(|_| ("jpg".to_string(), "locked".to_string())).collect();
        let b = ChangeBatch { renames, deletes: 0, total_files: 100 };
        assert!(suspicion(&b) > 0.8);
        assert!(should_freeze(&b));
    }

    #[test]
    fn a_few_normal_renames_dont_freeze() {
        let b = ChangeBatch { renames: vec![("txt".into(), "md".into()), ("jpg".into(), "png".into())], deletes: 1, total_files: 500 };
        assert!(!should_freeze(&b));
    }

    #[test]
    fn empty_is_safe() {
        assert_eq!(suspicion(&ChangeBatch { renames: vec![], deletes: 0, total_files: 0 }), 0.0);
    }
}
