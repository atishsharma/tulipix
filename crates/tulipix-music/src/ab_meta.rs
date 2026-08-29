//! `np.p7.music.audiobook-net` — audiobook identity: title, author, cover.
//!
//! Five resolution methods, best-first; results persist in `audiobook_meta`
//! (title, author) + `audiobook_covers` (cover path) so the library stays
//! consistent across restarts:
//!  1. Embedded tags of the first chapter — LibriVox-style rips carry
//!     album = book title, artist = author, and an archive.org link in the
//!     comment tag (which is also a direct cover source).
//!  2. Filename convention — `{title}_{nn}_{author}_{bitrate}.mp3`: the author
//!     token is whatever non-noise token every chapter file shares.
//!  3. LibriVox catalogue API — title lookup returns proper title, author and
//!     the archive.org identifier.
//!  4. iTunes audiobook search — commercial books; 600×600 art + author.
//!  5. Open Library search — last resort for title/author/cover.
//!
//! This lives here rather than in a section crate because BOTH front ends need
//! it: a book that resolves its cover in one build and shows a folder name in
//! the other is the same library answering the same question twice.

use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tulipix_core::proc::NoWindow;

/// Sidecar cover filenames tried inside a book folder, in order. Wider than
/// `folders::COVER_NAMES`: rips label their art `front.jpg` as often as not.
const SIDECAR: &[&str] = &[
    "cover.jpg", "cover.jpeg", "cover.png", "folder.jpg", "folder.png",
    "Cover.jpg", "Cover.png", "Folder.jpg", "front.jpg",
];

/// Folder basename → book title (the name shown until a real one resolves).
pub fn book_title(folder: &str) -> String {
    Path::new(folder)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| folder.to_string())
}

/// Folder basename → search query: separators to spaces, bracketed release
/// junk and common rip noise dropped ("Dune_1965_[64k MP3]" → "Dune 1965").
pub fn book_query(folder: &str) -> String {
    let base = book_title(folder);
    let mut out = String::with_capacity(base.len());
    let mut depth = 0i32;
    for c in base.chars() {
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(match c {
                '_' | '.' | '-' => ' ',
                _ => c,
            }),
            _ => {}
        }
    }
    let noise = [
        "unabridged", "abridged", "audiobook", "mp3", "m4b", "64k", "128k", "320k", "kbps",
    ];
    out.split_whitespace()
        .filter(|w| !noise.contains(&w.to_lowercase().as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The two side tables the lookup writes into. Created on demand — a library
/// that predates the feature has neither.
pub async fn ensure_tables(pool: &SqlitePool) {
    let _ = sqlx::query(
        "CREATE TABLE IF NOT EXISTS audiobook_covers (folder TEXT PRIMARY KEY, path TEXT NOT NULL)",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "CREATE TABLE IF NOT EXISTS audiobook_meta (folder TEXT PRIMARY KEY, author TEXT NOT NULL)",
    )
    .execute(pool)
    .await;
    // Added after the table shipped; the error on an existing column is the
    // expected outcome, not a failure.
    let _ = sqlx::query("ALTER TABLE audiobook_meta ADD COLUMN title TEXT")
        .execute(pool)
        .await;
}

/// folder → (title, author), whatever has been resolved so far.
pub async fn load_meta(pool: &SqlitePool) -> HashMap<String, (String, String)> {
    sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT folder, title, author FROM audiobook_meta",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(f, t, a)| (f, (t.unwrap_or_default(), a)))
    .collect()
}

/// folder → cover path (user-chosen, or the `_net.jpg` a lookup wrote).
pub async fn load_covers(pool: &SqlitePool) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>("SELECT folder, path FROM audiobook_covers")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Persist a user-chosen cover for one book. Outranks every lookup result.
pub async fn set_cover(pool: &SqlitePool, folder: &str, path: &str) -> Result<()> {
    ensure_tables(pool).await;
    sqlx::query(
        "INSERT INTO audiobook_covers (folder, path) VALUES (?,?) \
         ON CONFLICT(folder) DO UPDATE SET path = excluded.path",
    )
    .bind(folder)
    .bind(path)
    .execute(pool)
    .await?;
    Ok(())
}

/// Where extracted / downloaded book art is kept, shared by both front ends.
fn cover_cache_dir() -> Option<PathBuf> {
    let dir = tulipix_core::paths::config_dir()?.join("cache").join("abcover");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Stable per-folder cache stem, so the two builds name the same file.
fn folder_stem(folder: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(folder.as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Cover image for one book, best source first: the user's / a lookup's stored
/// choice, a sidecar image in the book folder, then the album art embedded in
/// the first chapter (extracted once with ffmpeg). `None` → the caller draws
/// its own placeholder.
///
/// `stored` is this folder's row from [`load_covers`], passed in so a caller
/// rendering a whole shelf reads that table once rather than once per book.
pub async fn cover_path(
    folder: &str,
    stored: Option<&str>,
    first_chapter: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = stored.map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let dir = PathBuf::from(folder);
    for name in SIDECAR {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    // No loose art file → pull the embedded album art out of the first chapter.
    let chapter = first_chapter?.to_path_buf();
    let out = cover_cache_dir()?.join(format!("{}.png", folder_stem(folder)));
    if out.exists() {
        return Some(out);
    }
    let ffmpeg = tulipix_core::thumbs::tool_bin("ffmpeg");
    let outc = out.clone();
    let _ = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&ffmpeg)
            .args(["-y", "-loglevel", "quiet", "-i"])
            .arg(&chapter)
            .args(["-map", "0:v:0", "-frames:v", "1"])
            .arg(&outc)
            .no_window()
            .status()
    })
    .await;
    out.exists().then_some(out)
}

/// Everything one lookup pass learns about a book.
#[derive(Default, Clone)]
pub struct AbInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    /// archive.org identifier — a direct cover source.
    pub archive_id: Option<String>,
    pub cover: Option<Vec<u8>>,
}

fn pick(dst: &mut Option<String>, src: Option<String>) {
    if dst.is_none() {
        *dst = src.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
}

/// Method 1 — embedded tags of the first chapter file. LibriVox rips carry
/// album = book title, artist = author, and the archive.org details URL in
/// the comment tag.
async fn info_from_tags(pool: &SqlitePool, folder: &str) -> AbInfo {
    let mut info = AbInfo::default();
    let Ok(Some(path)) = sqlx::query_scalar::<_, String>(
        "SELECT i.abs_path FROM items i JOIN track_meta tm ON tm.item_id = i.id \
         WHERE tm.folder = ? ORDER BY i.abs_path LIMIT 1",
    )
    .bind(folder)
    .fetch_optional(pool)
    .await
    else {
        return info;
    };
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(tulipix_core::thumbs::tool_bin("ffprobe"))
            .args(["-v", "quiet", "-print_format", "json", "-show_format"])
            .arg(&path)
            .no_window()
            .output()
    })
    .await
    .ok()
    .and_then(|r| r.ok());
    let Some(out) = out else { return info };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return info;
    };
    let tags = &v["format"]["tags"];
    let get = |k: &str| {
        tags.get(k)
            .or_else(|| tags.get(k.to_uppercase().as_str()))
            .and_then(|s| s.as_str())
            .map(String::from)
    };
    pick(&mut info.title, get("album"));
    pick(&mut info.author, get("artist").or_else(|| get("album_artist")));
    // "https://archive.org/details/<id>" anywhere in the comment.
    if let Some(c) = get("comment") {
        if let Some(idx) = c.find("archive.org/details/") {
            let id: String = c[idx + "archive.org/details/".len()..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '/' && *c != '"')
                .collect();
            if !id.is_empty() {
                info.archive_id = Some(id);
            }
        }
    }
    info
}

/// Method 2 — filename convention (`{title}_{nn}_{author}_{bitrate}`): the
/// author hint is a non-noise token shared by EVERY chapter file that isn't
/// part of the folder (title) name.
async fn author_hint_from_filenames(pool: &SqlitePool, folder: &str) -> Option<String> {
    let stems: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT i.abs_path FROM items i JOIN track_meta tm ON tm.item_id = i.id \
         WHERE tm.folder = ? LIMIT 40",
    )
    .bind(folder)
    .fetch_all(pool)
    .await
    .ok()?
    .into_iter()
    .filter_map(|p| {
        Path::new(&p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
    })
    .collect();
    if stems.len() < 2 {
        return None;
    }
    let title_l = book_title(folder).to_lowercase();
    let noise = ["64kb", "128kb", "mp3", "m4b", "librivox", "read", "by"];
    let is_candidate = |t: &str| {
        t.len() >= 3
            && !t.chars().any(|c| c.is_ascii_digit())
            && !noise.contains(&t)
            && !title_l.contains(t)
    };
    let first: Vec<String> = stems[0]
        .split(['_', '-', ' ', '.'])
        .filter(|t| is_candidate(t))
        .map(String::from)
        .collect();
    first.into_iter().find(|tok| {
        stems
            .iter()
            .all(|s| s.split(['_', '-', ' ', '.']).any(|t| t == tok))
    })
}

/// Method 3 — LibriVox catalogue: proper title, author and archive identifier.
async fn info_from_librivox(client: &reqwest::Client, query: &str) -> AbInfo {
    let mut info = AbInfo::default();
    let Ok(resp) = client
        .get("https://librivox.org/api/feed/audiobooks")
        .query(&[("format", "json"), ("limit", "1"), ("title", query)])
        .send()
        .await
    else {
        return info;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return info;
    };
    let Some(b) = v
        .get("books")
        .and_then(|b| b.as_array())
        .and_then(|a| a.first())
    else {
        return info;
    };
    pick(&mut info.title, b.get("title").and_then(|t| t.as_str()).map(String::from));
    if let Some(a) = b
        .get("authors")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
    {
        let name = format!(
            "{} {}",
            a.get("first_name").and_then(|s| s.as_str()).unwrap_or(""),
            a.get("last_name").and_then(|s| s.as_str()).unwrap_or("")
        );
        pick(&mut info.author, Some(name));
    }
    if let Some(u) = b.get("url_iarchive").and_then(|u| u.as_str()) {
        if let Some(idx) = u.find("archive.org/details/") {
            let id: String = u[idx + "archive.org/details/".len()..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '/')
                .collect();
            if !id.is_empty() {
                info.archive_id = Some(id);
            }
        }
    }
    info
}

async fn grab(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let b = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    // archive.org serves a tiny generic placeholder for unknown ids — skip it.
    (b.len() > 1500).then(|| b.to_vec())
}

/// Method 4 — iTunes audiobook search (art + author + title).
async fn info_from_itunes(client: &reqwest::Client, query: &str) -> AbInfo {
    let mut info = AbInfo::default();
    let Ok(resp) = client
        .get("https://itunes.apple.com/search")
        .query(&[("media", "audiobook"), ("limit", "1"), ("term", query)])
        .send()
        .await
    else {
        return info;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return info;
    };
    let Some(r) = v
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
    else {
        return info;
    };
    pick(&mut info.author, r.get("artistName").and_then(|a| a.as_str()).map(String::from));
    pick(&mut info.title, r.get("collectionName").and_then(|a| a.as_str()).map(String::from));
    if let Some(art) = r.get("artworkUrl100").and_then(|a| a.as_str()) {
        info.cover = grab(client, &art.replace("100x100", "600x600")).await;
    }
    info
}

/// Method 5 — Open Library search (title/author/cover fallback).
async fn info_from_openlibrary(client: &reqwest::Client, query: &str) -> AbInfo {
    let mut info = AbInfo::default();
    let Ok(resp) = client
        .get("https://openlibrary.org/search.json")
        .query(&[("q", query), ("limit", "1")])
        .send()
        .await
    else {
        return info;
    };
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return info;
    };
    let Some(doc) = v
        .get("docs")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
    else {
        return info;
    };
    pick(&mut info.title, doc.get("title").and_then(|t| t.as_str()).map(String::from));
    pick(
        &mut info.author,
        doc.get("author_name")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|a| a.as_str())
            .map(String::from),
    );
    if let Some(cid) = doc.get("cover_i").and_then(|c| c.as_i64()) {
        info.cover = grab(
            client,
            &format!("https://covers.openlibrary.org/b/id/{cid}-L.jpg"),
        )
        .await;
    }
    info
}

/// Full resolution chain for one folder. Local evidence (tags, filenames)
/// builds the query; the online methods fill whatever is still missing.
pub async fn resolve_info(pool: &SqlitePool, client: &reqwest::Client, folder: &str) -> AbInfo {
    // 1. Embedded tags.
    let mut info = info_from_tags(pool, folder).await;
    // 2. Filename author hint (used for the query even when tags had an author).
    let hint = author_hint_from_filenames(pool, folder).await;
    let title_q = info.title.clone().unwrap_or_else(|| book_query(folder));
    let author_q = info.author.clone().or(hint).unwrap_or_default();
    let full_q = if author_q.is_empty() {
        title_q.clone()
    } else {
        format!("{title_q} {author_q}")
    };
    // 3. LibriVox (these rips usually ARE LibriVox).
    if info.title.is_none() || info.author.is_none() || info.archive_id.is_none() {
        let lv = info_from_librivox(client, &title_q).await;
        pick(&mut info.title, lv.title);
        pick(&mut info.author, lv.author);
        if info.archive_id.is_none() {
            info.archive_id = lv.archive_id;
        }
    }
    // Cover from the archive identifier the moment we have one.
    if info.cover.is_none() {
        if let Some(id) = &info.archive_id {
            info.cover = grab(client, &format!("https://archive.org/services/img/{id}")).await;
        }
    }
    // 4. iTunes / 5. Open Library — only for what's still missing.
    if info.cover.is_none() || info.author.is_none() || info.title.is_none() {
        let it = info_from_itunes(client, &full_q).await;
        pick(&mut info.title, it.title);
        pick(&mut info.author, it.author);
        if info.cover.is_none() {
            info.cover = it.cover;
        }
    }
    if info.cover.is_none() || info.author.is_none() || info.title.is_none() {
        let ol = info_from_openlibrary(client, &full_q).await;
        pick(&mut info.title, ol.title);
        pick(&mut info.author, ol.author);
        if info.cover.is_none() {
            info.cover = ol.cover;
        }
    }
    info
}

/// Which of `folders` still need a lookup: no art anywhere, or art that an
/// earlier net pass fetched without ever resolving a real title (the improved
/// chain re-resolves those once). A user-chosen cover is never re-resolved.
pub fn needs_lookup(
    folders: &[String],
    covers: &HashMap<String, String>,
    meta: &HashMap<String, (String, String)>,
    has_local_art: impl Fn(&str) -> bool,
) -> Vec<String> {
    folders
        .iter()
        .filter(|f| {
            let net_cover = covers.get(*f).map(|p| p.ends_with("_net.jpg")).unwrap_or(false);
            let has_title = meta.get(*f).map(|(t, _)| !t.is_empty()).unwrap_or(false);
            (!covers.contains_key(*f) && !has_local_art(f.as_str())) || (net_cover && !has_title)
        })
        .cloned()
        .collect()
}

/// Resolve + persist title / author / cover for each folder. Returns true if
/// anything was written, so the caller knows whether to repaint.
///
/// A net-fetched cover (`*_net.jpg`) may be replaced by a better later
/// resolution; a user-chosen cover never is.
pub async fn resolve_and_store(pool: &SqlitePool, folders: &[String]) -> bool {
    if folders.is_empty() {
        return false;
    }
    ensure_tables(pool).await;
    let Some(out_dir) = cover_cache_dir() else {
        return false;
    };
    let client = tulipix_core::net::http().clone();
    let mut wrote = false;
    for folder in folders {
        let info = resolve_info(pool, &client, folder).await;
        if let Some(bytes) = &info.cover {
            let path = out_dir.join(format!("{}_net.jpg", folder_stem(folder)));
            if std::fs::write(&path, bytes).is_ok() {
                let existing: Option<String> =
                    sqlx::query_scalar("SELECT path FROM audiobook_covers WHERE folder = ?")
                        .bind(folder)
                        .fetch_optional(pool)
                        .await
                        .ok()
                        .flatten();
                let replace_ok = existing
                    .as_deref()
                    .map(|p| p.ends_with("_net.jpg"))
                    .unwrap_or(true);
                if replace_ok {
                    let _ = sqlx::query(
                        "INSERT OR REPLACE INTO audiobook_covers (folder, path) VALUES (?, ?)",
                    )
                    .bind(folder)
                    .bind(path.to_string_lossy().as_ref())
                    .execute(pool)
                    .await;
                }
            }
        }
        if info.title.is_some() || info.author.is_some() {
            let _ = sqlx::query(
                "INSERT INTO audiobook_meta (folder, author, title) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(folder) DO UPDATE SET author = ?2, title = ?3",
            )
            .bind(folder)
            .bind(info.author.clone().unwrap_or_default())
            .bind(info.title.clone())
            .execute(pool)
            .await;
        }
        wrote = true;
    }
    wrote
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strips_bracketed_junk_and_rip_noise() {
        assert_eq!(book_query("/b/Dune_1965_[64k MP3]"), "Dune 1965");
        assert_eq!(book_query("/b/The.Hobbit-unabridged"), "The Hobbit");
        // Nothing to strip is left exactly alone.
        assert_eq!(book_query("/b/Moby Dick"), "Moby Dick");
        assert_eq!(book_title("/b/Moby Dick"), "Moby Dick");
    }

    #[test]
    fn lookup_targets_only_the_books_that_need_one() {
        let folders: Vec<String> = ["/a", "/b", "/c", "/d"].iter().map(|s| s.to_string()).collect();
        let covers: HashMap<String, String> = [
            ("/b".to_string(), "/cache/abcover/ff_net.jpg".to_string()),
            ("/c".to_string(), "/home/me/mine.png".to_string()),
        ]
        .into_iter()
        .collect();
        // /b has net art but no resolved title -> retry. /c is the user's own
        // choice -> never. /d has a sidecar on disk -> nothing to fetch.
        let meta: HashMap<String, (String, String)> = HashMap::new();
        let got = needs_lookup(&folders, &covers, &meta, |f| f == "/d");
        assert_eq!(got, vec!["/a".to_string(), "/b".to_string()]);

        // Once /b has a real title, it stops being a target.
        let meta: HashMap<String, (String, String)> =
            [("/b".to_string(), ("Dune".to_string(), "Herbert".to_string()))]
                .into_iter()
                .collect();
        let got = needs_lookup(&folders, &covers, &meta, |f| f == "/d");
        assert_eq!(got, vec!["/a".to_string()]);
    }

    #[test]
    fn cache_stem_is_stable_and_per_folder() {
        assert_eq!(folder_stem("/books/dune"), folder_stem("/books/dune"));
        assert_ne!(folder_stem("/books/dune"), folder_stem("/books/hobbit"));
    }
}
