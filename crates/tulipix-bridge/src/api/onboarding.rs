//! First run — what the ten cards need from Rust.
//!
//! The flow itself is Dart (`shell/onboarding/`). Three things it cannot work
//! out on its own live here: whether to run at all, which folders are worth
//! offering, and the flag that stops it running a second time.
//!
//! **Nothing here writes a setting.** Every answer is committed through the
//! settings commands that already exist — `SetTheme`, `SaveProfile`,
//! `SetHomeLayout`, `LibAdd`, `Toggle`, `SetText` — so each setting keeps
//! exactly one write path and the first run cannot drift from the page that
//! edits the same value a week later.

use std::path::{Path, PathBuf};

use tulipix_core::cross::filter_chips::Chip;

use crate::api::shell::{load, save};

/// The flag that says it has been through.
///
/// `onboarded`, because the Slint build already reads that key: both front
/// ends run the first time, neither runs twice, and a machine set up in one
/// is set up in the other.
const FLAG: &str = "onboarded";

/// A folder the library card offers, with what it looks like it holds.
pub struct SuggestedFolder {
    pub path: String,
    /// `~/Pictures`. The home prefix is on every row and says nothing.
    pub label: String,
    /// The sections that would find something: "photos" | "videos" | "music"
    /// | "books", most-found first, so the card can lead with the likely one.
    pub sections: Vec<String>,
    /// Files of a kind those sections read.
    pub items: u32,
    /// The count stopped at the budget: there are at least `items`, probably
    /// more. The card says "4,000+" rather than a number that is simply wrong.
    pub capped: bool,
}

pub fn onboarding_needed() -> bool {
    !load().flag(FLAG, false)
}

/// Called when the last card closes — including when the whole flow was
/// skipped. Skipping setup is an answer, and being asked again next launch
/// would not be taking it.
pub fn onboarding_finish() {
    let mut s = load();
    s.flags.insert(FLAG.to_string(), true);
    save(s);
}

// ── suggestions ─────────────────────────────────────────────────────────────

/// Where people keep things. Both spellings of each, because macOS has Movies
/// where Linux has Videos, and whichever exists is the one offered.
const CANDIDATES: &[&str] =
    &["Pictures", "Photos", "Videos", "Movies", "Music", "Books", "Audiobooks", "Documents"];

const SECTIONS: [&str; 4] = ["photos", "videos", "music", "books"];

/// How many files one suggestion may cost.
///
/// A first run must not stall on a home folder with a hundred thousand photos
/// under it, and the number on the card is a reason to tick the box, not an
/// inventory. Three levels deep for the same reason: a library is arranged in
/// folders, not buried in them.
const BUDGET: u32 = 4_000;
const DEPTH: u32 = 3;

/// The folders worth offering: the usual places, minus anything already
/// watched — someone re-running this from Settings should not be offered what
/// they added last month.
pub fn onboarding_suggestions() -> Vec<SuggestedFolder> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let already = crate::api::settings::watched();
    let mut out = Vec::new();
    for name in CANDIDATES {
        let dir = home.join(name);
        if !dir.is_dir() || already.iter().any(|w| w == &dir) {
            continue;
        }
        if let Some(found) = look(&dir) {
            out.push(found);
        }
    }
    out
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// `None` for a folder that holds nothing Tulipix reads — an empty ~/Music is
/// not worth a row that says "0 items".
fn look(dir: &Path) -> Option<SuggestedFolder> {
    let mut counts = [0u32; 4];
    let mut seen = 0u32;
    walk(dir, DEPTH, &mut seen, &mut counts);
    let total: u32 = counts.iter().sum();
    if total == 0 {
        return None;
    }
    let mut ranked: Vec<(usize, u32)> =
        counts.iter().copied().enumerate().filter(|(_, n)| *n > 0).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    Some(SuggestedFolder {
        path: dir.display().to_string(),
        label: crate::api::settings::tildeish(dir),
        sections: ranked.iter().map(|(i, _)| SECTIONS[*i].to_string()).collect(),
        items: total,
        capped: seen >= BUDGET,
    })
}

fn walk(dir: &Path, depth: u32, seen: &mut u32, counts: &mut [u32; 4]) {
    if depth == 0 || *seen >= BUDGET {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if *seen >= BUDGET {
            return;
        }
        let p = e.path();
        // A dotfolder on a first run is a cache, a trash or a version-control
        // directory. None of them is a library, and ~/.cache alone would spend
        // the whole budget.
        if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        if p.is_dir() {
            walk(&p, depth - 1, seen, counts);
            continue;
        }
        *seen += 1;
        if let Some(slot) = slot_for(&p) {
            counts[slot] += 1;
        }
    }
}

/// Which of the four sections would read this file, or `None`.
///
/// The extension sets are `filter_chips::Chip`'s, which is what the chip bar,
/// search and storage insights already agree on — a second list here would be
/// a second answer to "is this a photo".
fn slot_for(p: &Path) -> Option<usize> {
    let name = p.to_string_lossy();
    if Chip::Photos.matches(&name) {
        Some(0)
    } else if Chip::Videos.matches(&name) {
        Some(1)
    } else if Chip::Audio.matches(&name) {
        Some(2)
    } else if Chip::Documents.matches(&name) || Chip::Pdfs.matches(&name) {
        Some(3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_lands_in_one_section_each() {
        assert_eq!(slot_for(Path::new("/x/a.jpg")), Some(0));
        assert_eq!(slot_for(Path::new("/x/a.mkv")), Some(1));
        assert_eq!(slot_for(Path::new("/x/a.flac")), Some(2));
        assert_eq!(slot_for(Path::new("/x/a.epub")), Some(3));
        assert_eq!(slot_for(Path::new("/x/a.pdf")), Some(3));
        assert_eq!(slot_for(Path::new("/x/notes.sqlite")), None);
    }

    /// The card leads with the section that found the most, so a Pictures
    /// folder with a few clips in it still reads as photos.
    #[test]
    fn the_busiest_section_leads() {
        let dir = std::env::temp_dir().join("tulipix-onboard-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cache")).unwrap();
        for n in 0..5 {
            std::fs::write(dir.join(format!("p{n}.jpg")), b"x").unwrap();
        }
        std::fs::write(dir.join("clip.mp4"), b"x").unwrap();
        // Inside a dotfolder, so it must not be counted at all.
        std::fs::write(dir.join(".cache/hidden.jpg"), b"x").unwrap();

        let found = look(&dir).expect("the folder holds something");
        assert_eq!(found.sections, vec!["photos", "videos"]);
        assert_eq!(found.items, 6, "the dotfolder's jpg is not counted");
        assert!(!found.capped);

        let empty = std::env::temp_dir().join("tulipix-onboard-empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(look(&empty).is_none(), "nothing to read is no suggestion");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
