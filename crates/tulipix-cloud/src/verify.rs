//! `np.p5.cloud.verify` — integrity: checksum verify after transfer + dedupe.
//!
//! `rclone check` compares two trees by hash (or size+modtime when a backend
//! lacks hashes); `rclone dedupe` finds and resolves duplicate-named objects on
//! a single remote. Argv construction + the dedupe-mode whitelist live here;
//! the subprocess spawn lives in the app layer.

/// `rclone check <src> <dst>` — hash-verify two trees. `one_way` skips the
/// "extra files on dst" check (post-upload verification only cares that what we
/// sent arrived intact). `--checkers` bounds concurrency.
pub fn check_args(src: &str, dst: &str, one_way: bool, checkers: u32) -> Vec<String> {
    let mut a = vec!["check".into(), src.into(), dst.into()];
    if one_way { a.push("--one-way".into()); }
    if checkers > 0 { a.push("--checkers".into()); a.push(checkers.to_string()); }
    a
}

/// rclone dedupe resolution modes. `Interactive` is excluded — the app is
/// non-interactive, every run must pick a concrete policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DedupeMode { Skip, First, Newest, Oldest, Largest, Smallest, Rename }

impl DedupeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DedupeMode::Skip => "skip",
            DedupeMode::First => "first",
            DedupeMode::Newest => "newest",
            DedupeMode::Oldest => "oldest",
            DedupeMode::Largest => "largest",
            DedupeMode::Smallest => "smallest",
            DedupeMode::Rename => "rename",
        }
    }
    pub fn parse(s: &str) -> Option<DedupeMode> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "skip" => DedupeMode::Skip,
            "first" => DedupeMode::First,
            "newest" => DedupeMode::Newest,
            "oldest" => DedupeMode::Oldest,
            "largest" => DedupeMode::Largest,
            "smallest" => DedupeMode::Smallest,
            "rename" => DedupeMode::Rename,
            _ => return None,
        })
    }
}

/// `rclone dedupe --dedupe-mode <mode> <remote:path>`.
pub fn dedupe_args(target: &str, mode: DedupeMode) -> Vec<String> {
    vec!["dedupe".into(), "--dedupe-mode".into(), mode.as_str().into(), target.into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_argv() {
        let a = check_args("gdrive:docs", "/local/docs", true, 8);
        assert_eq!(a[..3], ["check", "gdrive:docs", "/local/docs"]);
        assert!(a.contains(&"--one-way".to_string()));
        assert!(a.windows(2).any(|w| w == ["--checkers", "8"]));
        let b = check_args("a:", "b:", false, 0);
        assert!(!b.contains(&"--one-way".to_string()));
        assert!(!b.contains(&"--checkers".to_string()));
    }

    #[test]
    fn dedupe_modes_roundtrip() {
        for m in ["skip", "first", "newest", "oldest", "largest", "smallest", "rename"] {
            assert_eq!(DedupeMode::parse(m).unwrap().as_str(), m);
        }
        assert!(DedupeMode::parse("interactive").is_none());
        let a = dedupe_args("gdrive:photos", DedupeMode::Newest);
        assert_eq!(a, ["dedupe", "--dedupe-mode", "newest", "gdrive:photos"]);
    }
}
