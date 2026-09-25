//! The watched-folder list: `watched_folders.json` in the config folder, a
//! JSON array of paths. Every section reads it and Settings › Libraries shows
//! it, and this module is the only thing that touches the file.
//!
//! Seven copies of load-and-save used to live around the app, each writing
//! with a plain `fs::write` (truncate, then fill) and each reading any failure
//! as "no folders". A read that landed between the truncate and the fill saw
//! an empty file, Libraries showed nothing, and a writer that had read that
//! empty list saved it back with only its own folder in it. So:
//!
//! - writes go to a temp file and are renamed over, so a reader sees the old
//!   list or the new one, never half of one;
//! - every read-change-write holds one lock, so two writers cannot each save
//!   the list they read before the other's change;
//! - the list as it was before each write is kept as `.bak`, and a read that
//!   finds the file unreadable falls back to it;
//! - nothing is written over a file that could not be read — that is how an
//!   unreadable list became a lost one.
//!
//! One process owns the file; the lock is in-process.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What the file on disk says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Read and parsed.
    Ok,
    /// No file: a first run, or nothing ever watched.
    Fresh,
    /// There, but not a list: cut short, or edited by hand into nonsense.
    Unreadable,
}

static LOCK: Mutex<()> = Mutex::new(());

pub fn path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

fn bak_of(p: &Path) -> PathBuf {
    p.with_extension("json.bak")
}

fn parse(body: &str) -> Option<Vec<PathBuf>> {
    serde_json::from_str::<Vec<String>>(body)
        .ok()
        .map(|v| v.into_iter().map(PathBuf::from).collect())
}

fn read_at(p: &Path) -> (Vec<PathBuf>, Health) {
    match std::fs::read_to_string(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), Health::Fresh),
        Err(_) => (Vec::new(), Health::Unreadable),
        Ok(body) => match parse(&body) {
            Some(v) => (v, Health::Ok),
            None => (Vec::new(), Health::Unreadable),
        },
    }
}

fn load_at(p: &Path) -> Vec<PathBuf> {
    match read_at(p) {
        (v, Health::Unreadable) => {
            let bak = std::fs::read_to_string(bak_of(p)).ok().and_then(|b| parse(&b));
            tracing::warn!(file = %p.display(), backup = bak.is_some(), "watched folders unreadable");
            bak.unwrap_or(v)
        }
        (v, _) => v,
    }
}

fn write_at(p: &Path, list: &[PathBuf]) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The list as it was, kept once it is one worth keeping.
    if let (old, Health::Ok) = read_at(p)
        && !old.is_empty()
    {
        let _ = std::fs::copy(p, bak_of(p));
    }
    let out: Vec<String> = list.iter().map(|x| x.to_string_lossy().into_owned()).collect();
    let body = serde_json::to_vec_pretty(&out).map_err(std::io::Error::other)?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, p)
}

/// Change the list under the lock. `f` says whether it changed anything; only
/// then is it written. Refused while the file cannot be read.
fn update_at(p: &Path, f: impl FnOnce(&mut Vec<PathBuf>) -> bool) -> Result<bool, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut list, health) = read_at(p);
    if health == Health::Unreadable {
        return Err("The folder list could not be read, so it was left alone. \
                    Restore it from the backup in Settings › Libraries."
            .into());
    }
    if !f(&mut list) {
        return Ok(false);
    }
    write_at(p, &list).map_err(|e| format!("Could not save the folder list: {e}"))?;
    Ok(true)
}

/// The watched folders. An unreadable file reads as its backup, then as
/// nothing; [`health`] says which happened.
pub fn load() -> Vec<PathBuf> {
    path().map(|p| load_at(&p)).unwrap_or_default()
}

/// What the file itself says, without falling back to the backup.
pub fn health() -> Health {
    path().map(|p| read_at(&p).1).unwrap_or(Health::Fresh)
}

/// When the list was last saved, Unix seconds; 0 when never.
pub fn saved_at() -> i64 {
    path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The backup: the list as it was before the last save. None when there is
/// no usable one.
pub fn backup() -> Option<Vec<PathBuf>> {
    let p = path()?;
    parse(&std::fs::read_to_string(bak_of(&p)).ok()?)
}

/// Watch `dir`. False when it already was, or the list could not be saved.
pub fn add(dir: &Path) -> bool {
    let Some(p) = path() else { return false };
    update_at(&p, |l| {
        if l.iter().any(|x| x == dir) {
            return false;
        }
        l.push(dir.to_path_buf());
        true
    })
    .unwrap_or_else(|e| {
        tracing::warn!("{e}");
        false
    })
}

/// Stop watching `dir`. False when it was not watched.
pub fn remove(dir: &Path) -> Result<bool, String> {
    let p = path().ok_or("no config folder")?;
    update_at(&p, |l| {
        let before = l.len();
        l.retain(|x| x != dir);
        l.len() != before
    })
}

/// Keep only the folders `keep` says yes to.
pub fn retain(keep: impl Fn(&Path) -> bool) -> Result<bool, String> {
    let p = path().ok_or("no config folder")?;
    update_at(&p, |l| {
        let before = l.len();
        l.retain(|x| keep(x));
        l.len() != before
    })
}

/// Replace the whole list: a Settings restore.
pub fn set(list: Vec<PathBuf>) -> Result<(), String> {
    let p = path().ok_or("no config folder")?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    write_at(&p, &list).map_err(|e| format!("Could not save the folder list: {e}"))
}

/// Put the backup back. The list it replaces becomes the new backup, if it
/// was readable, so a restore can itself be undone.
pub fn restore_backup() -> Result<usize, String> {
    let p = path().ok_or("no config folder")?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let list = std::fs::read_to_string(bak_of(&p))
        .ok()
        .and_then(|b| parse(&b))
        .ok_or("There is no backup of the folder list.")?;
    write_at(&p, &list).map_err(|e| format!("Could not save the folder list: {e}"))?;
    Ok(list.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(p: &Path) -> Vec<PathBuf> {
        read_at(p).0
    }

    #[test]
    fn a_missing_file_is_fresh_and_empty() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_at(&d.path().join("w.json")), (Vec::new(), Health::Fresh));
    }

    #[test]
    fn writes_round_trip_and_keep_the_last_list() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("w.json");
        update_at(&p, |l| {
            l.push("/a".into());
            true
        })
        .unwrap();
        update_at(&p, |l| {
            l.push("/b".into());
            true
        })
        .unwrap();
        assert_eq!(list(&p), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(parse(&std::fs::read_to_string(bak_of(&p)).unwrap()).unwrap(), vec![PathBuf::from("/a")]);
        assert!(!p.with_extension("json.tmp").exists());
    }

    /// The bug: a file caught empty mid-write read as "no folders", and the
    /// next writer saved that. Now it reads as the backup, and nothing writes
    /// over it.
    #[test]
    fn a_cut_short_file_reads_as_its_backup_and_is_not_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("w.json");
        update_at(&p, |l| {
            l.extend(["/a".into(), "/b".into()]);
            true
        })
        .unwrap();
        update_at(&p, |l| {
            l.push("/c".into());
            true
        })
        .unwrap();
        std::fs::write(&p, "").unwrap();
        assert_eq!(read_at(&p).1, Health::Unreadable);
        assert_eq!(load_at(&p), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert!(update_at(&p, |l| {
            l.push("/d".into());
            true
        })
        .is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
    }

    #[test]
    fn an_unchanged_list_is_not_written() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("w.json");
        assert_eq!(update_at(&p, |_| false), Ok(false));
        assert!(!p.exists());
    }
}
