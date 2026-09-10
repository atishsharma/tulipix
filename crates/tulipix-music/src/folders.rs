//! `np.p4.music.folders` — folder browse view (disk hierarchy as-found).
//!
//! Groups tracks by their on-disk `folder` and resolves a per-folder cover
//! from the conventional sidecar names (folder.jpg / cover.jpg / front.png …),
//! falling back to embedded art. The cover-name precedence is pure logic and
//! unit-tested; the listing query reads `track_meta.folder`.

use anyhow::Result;
use sqlx::SqlitePool;

/// Sidecar cover filenames in precedence order.
pub const COVER_NAMES: &[&str] = &["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png", "album.jpg"];

/// Pick the best sidecar cover from a directory listing, honoring precedence.
/// Case-insensitive. Returns `None` → caller falls back to embedded art.
pub fn pick_cover(files_in_dir: &[String]) -> Option<&String> {
    for want in COVER_NAMES {
        if let Some(f) = files_in_dir.iter().find(|f| {
            std::path::Path::new(f).file_name().and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(want)).unwrap_or(false)
        }) {
            return Some(f);
        }
    }
    None
}

/// Folders that contain tracks, with track counts, alphabetical.
///
/// Audiobook-flagged tracks are excluded, so a book's folder never appears in
/// My Music's Folders tab. The section-tag filter the callers apply on top of
/// this only knows the folder the user actually picked; a shelf pointed at a
/// PARENT directory has one sub-folder per book, and none of those are keys in
/// that map — which is exactly how books ended up listed as music folders.
pub async fn list(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT folder, COUNT(*) FROM track_meta
         JOIN items ON items.id = track_meta.item_id
         WHERE items.missing_since IS NULL AND folder IS NOT NULL
           AND COALESCE(track_meta.is_audiobook, 0) = 0
         GROUP BY folder ORDER BY folder COLLATE NOCASE",
    ).fetch_all(pool).await?)
}

/// Tracks in one folder, ordered by disc/track then path.
pub async fn tracks_in(pool: &SqlitePool, folder: &str) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT items.id, items.abs_path FROM track_meta
         JOIN items ON items.id = track_meta.item_id
         WHERE track_meta.folder = ? AND items.missing_since IS NULL
         ORDER BY track_meta.disc_no, track_meta.track_no, items.abs_path",
    ).bind(folder).fetch_all(pool).await?)
}


/// One folder in the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderNode {
    /// Absolute path, which is also this node's key.
    pub path: String,
    /// What to show: the last segment, or the whole collapsed run of segments
    /// when a chain of single-child folders was folded into one row.
    pub name: String,
    /// Tracks sitting directly in this folder.
    pub direct: i64,
    /// Tracks in this folder and everything beneath it.
    pub total: i64,
    pub has_children: bool,
}

/// Path separator. Folders are stored as the OS wrote them, so this splits on
/// both: a library copied from Windows keeps its backslashes in the column.
fn segments(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
}

/// The immediate children of `under`, or the top of the tree when it is empty.
///
/// A folder view that is flat is the one thing a folder view must not be. This
/// is the nesting: one level at a time, so a library on a slow disk does not
/// pay for a tree nobody expanded.
///
/// Single-child chains are collapsed the way a file manager collapses them --
/// `/home/you/Music` is one row, not three empty ones -- because the useful
/// root of a music library is never the filesystem root. A folder that holds
/// tracks of its own is never collapsed away, since it is somewhere you can go.
pub async fn children(pool: &SqlitePool, under: &str) -> Result<Vec<FolderNode>> {
    let all = list(pool).await?;
    Ok(children_of(&all, under))
}

/// The tree walk itself, over an already-loaded folder list. Pure, so the
/// collapsing rule is testable without a database.
fn children_of(all: &[(String, i64)], under: &str) -> Vec<FolderNode> {
    let mut at = under.to_string();
    // Bounded by path depth: every pass either returns or descends one level.
    for _ in 0..64 {
        let depth = segments(&at).len();
        let here = segments(&at);
        let mut kids: Vec<FolderNode> = Vec::new();

        for (folder, count) in all {
            let segs = segments(folder);
            // Under `at`, with at least one more segment to name a child.
            if segs.len() <= depth || segs[..depth] != here[..] {
                continue;
            }
            let child_path = rebuild(folder, depth + 1);
            // By index, not by holding a `&mut` from `find`: a reference taken
            // out of one arm of a match that also pushes is a borrow the
            // compiler is right to refuse.
            let idx = match kids.iter().position(|k| k.path == child_path) {
                Some(i) => i,
                None => {
                    kids.push(FolderNode {
                        name: segs[depth].to_string(),
                        path: child_path,
                        direct: 0,
                        total: 0,
                        has_children: false,
                    });
                    kids.len() - 1
                }
            };
            kids[idx].total += count;
            if segs.len() == depth + 1 {
                kids[idx].direct += count;
            } else {
                kids[idx].has_children = true;
            }
        }

        // One child with nothing of its own is a corridor, not a room: descend
        // through it rather than making the user click through it.
        if kids.len() == 1 && kids[0].direct == 0 && kids[0].has_children {
            at = kids[0].path.clone();
            continue;
        }

        // Names are relative to where the caller asked from, so a collapsed
        // run reads as one row -- "home/you/Music" -- rather than hiding the
        // descent that just happened.
        let from = segments(under).len();
        for k in kids.iter_mut() {
            let shown = segments(&k.path);
            k.name = shown[from.min(shown.len())..].join("/");
        }
        kids.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        return kids;
    }
    Vec::new()
}

/// The first `n` segments of `path`, written the way `path` writes them.
///
/// Not `segments().join("/")`: that loses a leading slash on unix and turns a
/// Windows library's backslashes into forward ones, and the result is used as
/// a key against the `folder` column.
fn rebuild(path: &str, n: usize) -> String {
    let mut count = 0;
    for (i, c) in path.char_indices() {
        if c != '/' && c != '\\' {
            continue;
        }
        // A leading separator opens the path rather than dividing it, and a
        // run of separators is one boundary.
        if i == 0 || matches!(path[..i].chars().last(), Some('/') | Some('\\')) {
            continue;
        }
        count += 1;
        if count == n {
            return path[..i].to_string();
        }
    }
    path.to_string()
}

/// Every track in `prefix` and everything beneath it, in path order.
///
/// What "play folder and below" plays: a whole discography directory goes to
/// the queue in one action instead of one album at a time.
pub async fn tracks_under(pool: &SqlitePool, prefix: &str) -> Result<Vec<i64>> {
    if prefix.is_empty() {
        return Ok(Vec::new());
    }
    // The trailing separator matters: without it `/m/Air` also matches
    // `/m/Airbag`, and playing one artist would play the next one too.
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT items.id FROM track_meta
         JOIN items ON items.id = track_meta.item_id
         WHERE items.missing_since IS NULL
           AND COALESCE(track_meta.is_audiobook, 0) = 0
           AND (track_meta.folder = ? OR track_meta.folder LIKE ? ESCAPE '\\'
                OR track_meta.folder LIKE ? ESCAPE '\\')
         ORDER BY track_meta.folder COLLATE NOCASE,
                  track_meta.disc_no, track_meta.track_no, items.abs_path",
    )
    .bind(prefix)
    .bind(format!("{}/%", like_escape(prefix)))
    .bind(format!("{}\\%", like_escape(prefix)))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Escape a path for use as a `LIKE` prefix. A folder called `100% Mixes` is
/// otherwise a wildcard that matches every folder in the library.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use crate::{scan, tags::TrackTags};


    #[test]
    fn the_tree_collapses_corridors_and_stops_at_rooms() {
        let all = vec![
            ("/home/you/Music/Air/Talkie Walkie".to_string(), 10i64),
            ("/home/you/Music/Air/Moon Safari".to_string(), 10),
            ("/home/you/Music/Burial".to_string(), 8),
        ];
        // Nobody wants to click through /, home and you to reach their music.
        let roots = children_of(&all, "");
        assert_eq!(roots.len(), 2, "Music has two children, so that is the room");
        assert_eq!(roots[0].name, "home/you/Music/Air");
        assert_eq!(roots[0].total, 20);
        assert_eq!(roots[0].direct, 0);
        assert!(roots[0].has_children);
        assert_eq!(roots[1].name, "home/you/Music/Burial");
        assert_eq!((roots[1].direct, roots[1].has_children), (8, false));

        let inside = children_of(&all, "/home/you/Music/Air");
        assert_eq!(inside.len(), 2);
        assert_eq!(inside[0].name, "Moon Safari");
        assert_eq!(inside[0].path, "/home/you/Music/Air/Moon Safari");
        assert_eq!(inside[0].direct, 10);
        assert!(!inside[0].has_children);

        // A folder with tracks of its own is a room even with one child.
        let mixed = vec![
            ("/m/Band".to_string(), 2i64),
            ("/m/Band/Live".to_string(), 5),
        ];
        let r = children_of(&mixed, "");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].direct, r[0].total), (2, 7));
        assert_eq!(children_of(&mixed, "/m/Band").len(), 1);
        assert!(children_of(&mixed, "/m/Band/Live").is_empty());
    }

    #[test]
    fn rebuild_keeps_the_separators_it_was_given() {
        assert_eq!(rebuild("/m/Band/Disc", 1), "/m");
        assert_eq!(rebuild("/m/Band/Disc", 2), "/m/Band");
        assert_eq!(rebuild("/m/Band/Disc", 9), "/m/Band/Disc");
        assert_eq!(rebuild("C:\\Music\\Band", 1), "C:");
        assert_eq!(rebuild("C:\\Music\\Band", 2), "C:\\Music");
    }

    #[tokio::test]
    async fn playing_a_folder_and_below_stops_at_the_separator() {
        let (_t, pool) = open_pool().await;
        for p in ["/m/Air/a.flac", "/m/Air/Live/b.flac", "/m/Airbag/c.flac"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)").bind(p).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(&pool).await.unwrap();
            scan::upsert_track(&pool, id, p, &TrackTags::default()).await.unwrap();
        }
        // Without the trailing separator in the LIKE, /m/Air would swallow
        // /m/Airbag and playing one artist would play the next one too.
        assert_eq!(tracks_under(&pool, "/m/Air").await.unwrap().len(), 2);
        assert_eq!(tracks_under(&pool, "/m/Airbag").await.unwrap().len(), 1);
        assert!(tracks_under(&pool, "").await.unwrap().is_empty());
    }

    #[test]
    fn cover_precedence() {
        let files = vec!["/a/folder.jpg".to_string(), "/a/cover.png".to_string(), "/a/song.flac".to_string()];
        // cover.png beats folder.jpg by precedence order
        assert_eq!(pick_cover(&files), Some(&"/a/cover.png".to_string()));
        assert!(pick_cover(&["/a/song.flac".to_string()]).is_none());
    }

    #[tokio::test]
    async fn folders_grouped() {
        let (_t, pool) = open_pool().await;
        for p in ["/m/Band/Disc/1.flac", "/m/Band/Disc/2.flac"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)").bind(p).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(&pool).await.unwrap();
            scan::upsert_track(&pool, id, p, &TrackTags::default()).await.unwrap();
        }
        let f = list(&pool).await.unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0], ("/m/Band/Disc".to_string(), 2));
        assert_eq!(tracks_in(&pool, "/m/Band/Disc").await.unwrap().len(), 2);

        // A book's folder belongs to the Audiobooks shelf, never to this list.
        let p = "/books/dune/ch01.mp3";
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)").bind(p).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(&pool).await.unwrap();
        scan::upsert_track(&pool, id, p, &TrackTags::default()).await.unwrap();
        assert_eq!(list(&pool).await.unwrap().len(), 2, "unflagged, it is just a folder");
        sqlx::query("UPDATE track_meta SET is_audiobook = 1 WHERE item_id = ?")
            .bind(id).execute(&pool).await.unwrap();
        assert_eq!(list(&pool).await.unwrap(), vec![("/m/Band/Disc".to_string(), 2)]);
    }
}
