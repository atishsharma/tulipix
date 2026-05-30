//! Sibling subtitle autopick — .srt / .vtt / .ass next to the video.
//!
//! Scoring (higher = better):
//!   * exact stem match: +100
//!   * stem prefix match (e.g. `Movie (2020).en.srt`): +60
//!   * preferred-language hit in filename: +20 per language tier
//!   * format priority: ass > srt > vtt > sub (so styled subs win when both exist)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SUB_EXTS: &[&str] = &["ass", "srt", "vtt", "sub", "ssa"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleCandidate {
    pub path: PathBuf,
    pub language: Option<String>,
    pub score: i32,
    pub ext: String,
}

pub fn is_subtitle(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            SUB_EXTS.iter().any(|x| **x == e)
        })
        .unwrap_or(false)
}

/// Find every subtitle file adjacent to `video` and rank by suitability.
pub fn find_siblings(video: &Path, preferred_langs: &[&str]) -> Vec<SubtitleCandidate> {
    let Some(dir) = video.parent() else { return Vec::new(); };
    let Some(stem) = video.file_stem().and_then(|s| s.to_str()) else { return Vec::new(); };
    let stem_lc = stem.to_ascii_lowercase();

    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new(); };
    let mut out: Vec<SubtitleCandidate> = Vec::new();
    for entry in read.flatten() {
        let p = entry.path();
        if !p.is_file() || !is_subtitle(&p) { continue; }
        let Some(name) = p.file_stem().and_then(|s| s.to_str()) else { continue; };
        let name_lc = name.to_ascii_lowercase();
        let mut score = 0i32;
        if name_lc == stem_lc { score += 100; }
        else if name_lc.starts_with(&stem_lc) { score += 60; }
        else { continue; }

        let lang = extract_lang(&name_lc, &stem_lc);
        for (rank, want) in preferred_langs.iter().enumerate() {
            if lang.as_deref() == Some(want) {
                score += 20 - rank as i32 * 5;
                break;
            }
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        score += match ext.as_str() {
            "ass" | "ssa" => 5,
            "srt" => 3,
            "vtt" => 2,
            _ => 0,
        };
        out.push(SubtitleCandidate { path: p, language: lang, score, ext });
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    out
}

/// `Movie.en.srt` with stem `Movie` → `Some("en")`. Otherwise `None`.
fn extract_lang(name_lc: &str, stem_lc: &str) -> Option<String> {
    let rest = name_lc.strip_prefix(stem_lc)?;
    let rest = rest.trim_start_matches(['.', '_', '-', ' ']);
    if rest.is_empty() { return None; }
    let tag = rest.split(|c: char| matches!(c, '.' | '_' | '-' | ' ')).next()?;
    if (2..=5).contains(&tag.len()) && tag.chars().all(|c| c.is_ascii_lowercase()) {
        Some(tag.to_string())
    } else { None }
}

/// Convenience: best candidate for a video given a preferred-language list.
pub fn autopick(video: &Path, preferred_langs: &[&str]) -> Option<SubtitleCandidate> {
    find_siblings(video, preferred_langs).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    #[test]
    fn ext_recognises_common_formats() {
        for e in ["srt", "VTT", "ass", "ssa"] {
            assert!(is_subtitle(Path::new(&format!("/x.{e}"))));
        }
        assert!(!is_subtitle(Path::new("/x.txt")));
    }

    #[test]
    fn exact_match_outranks_prefix_match() {
        let tmp = tempfile::tempdir().unwrap();
        let video = touch(tmp.path(), "Show.S01E02.mkv");
        let exact = touch(tmp.path(), "Show.S01E02.srt");
        let langed = touch(tmp.path(), "Show.S01E02.en.srt");
        let list = find_siblings(&video, &["en"]);
        assert!(list.len() >= 2);
        // exact (no lang tag) still beats prefix-with-tag because exact is 100, prefix is 60.
        assert_eq!(list[0].path, exact);
        assert!(list.iter().any(|c| c.path == langed && c.language.as_deref() == Some("en")));
    }

    #[test]
    fn lang_preference_orders_prefix_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let video = touch(tmp.path(), "M.mkv");
        let en = touch(tmp.path(), "M.en.srt");
        let _es = touch(tmp.path(), "M.es.srt");
        let list = find_siblings(&video, &["en", "es"]);
        assert_eq!(list[0].path, en);
    }

    #[test]
    fn unrelated_subtitles_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let video = touch(tmp.path(), "M.mkv");
        let _ = touch(tmp.path(), "Other.srt");
        assert!(find_siblings(&video, &["en"]).is_empty());
    }

    #[test]
    fn autopick_returns_best() {
        let tmp = tempfile::tempdir().unwrap();
        let video = touch(tmp.path(), "M.mkv");
        let _vtt = touch(tmp.path(), "M.vtt");
        let srt = touch(tmp.path(), "M.srt");
        let pick = autopick(&video, &[]).unwrap();
        assert_eq!(pick.path, srt); // srt beats vtt in format priority at equal score
    }
}
