//! `np.p4.music.audiobooks` — chapter nav, position-remember, 0.5×–3× speed
//! without pitch shift.
//!
//! Marks a track as an audiobook, persists resume position + playback speed in
//! `audiobook_progress`, and owns the mpv options that change tempo while
//! preserving pitch (the `scaletempo2` path).

use anyhow::Result;
use sqlx::SqlitePool;

pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 3.0;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn clamp_speed(s: f64) -> f64 { s.clamp(MIN_SPEED, MAX_SPEED) }

/// mpv options for pitch-preserving speed change.
pub fn mpv_speed_options(speed: f64) -> Vec<String> {
    vec![
        "--audio-pitch-correction=yes".to_string(),
        format!("--speed={}", clamp_speed(speed)),
    ]
}

pub async fn mark_audiobook(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET is_audiobook = 1 WHERE item_id = ?").bind(item_id).execute(pool).await?;
    Ok(())
}

/// Save resume position + speed.
pub async fn save_progress(pool: &SqlitePool, item_id: i64, position_s: f64, speed: f64) -> Result<()> {
    sqlx::query(
        "INSERT INTO audiobook_progress (item_id, position_s, speed, updated) VALUES (?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET position_s=excluded.position_s, speed=excluded.speed, updated=excluded.updated",
    ).bind(item_id).bind(position_s).bind(clamp_speed(speed)).bind(now()).execute(pool).await?;
    Ok(())
}

/// Resume position + speed, defaulting to start / 1.0× if unseen.
pub async fn resume(pool: &SqlitePool, item_id: i64) -> Result<(f64, f64)> {
    let row: Option<(f64, f64)> = sqlx::query_as("SELECT position_s, speed FROM audiobook_progress WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await?;
    Ok(row.unwrap_or((0.0, 1.0)))
}

/// Chapter-level book state (np: chapter-resume, user decree 2026-07-12 — a
/// chapter always restarts from 0:00; bookmarks cover in-chapter positions):
/// returns (ids with any progress row = "listened" colour, most-recently
/// updated id = the CURRENT chapter, the Resume target / red row).
pub async fn chapter_states(pool: &SqlitePool, ids: &[i64]) -> Result<(Vec<i64>, Option<i64>)> {
    if ids.is_empty() { return Ok((Vec::new(), None)); }
    let ph = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT item_id, updated FROM audiobook_progress WHERE item_id IN ({ph})");
    let mut q = sqlx::query_as::<_, (i64, i64)>(&sql);
    for id in ids { q = q.bind(id); }
    let rows: Vec<(i64, i64)> = q.fetch_all(pool).await?;
    let current = rows.iter().max_by_key(|(_, u)| *u).map(|(id, _)| *id);
    Ok((rows.into_iter().map(|(id, _)| id).collect(), current))
}

/// Save a position bookmark for an audiobook (np.p5.music.audiobook-chapters).
pub async fn add_bookmark(pool: &SqlitePool, item_id: i64, position_s: f64, label: &str) -> Result<()> {
    sqlx::query("INSERT INTO audiobook_bookmarks (item_id, position_s, label, created) VALUES (?,?,?,?)")
        .bind(item_id).bind(position_s)
        .bind(if label.is_empty() { None } else { Some(label) })
        .bind(now()).execute(pool).await?;
    Ok(())
}

/// Ordered (position_s, label) bookmarks for an audiobook, earliest first.
pub async fn bookmarks(pool: &SqlitePool, item_id: i64) -> Result<Vec<(f64, String)>> {
    let rows: Vec<(f64, Option<String>)> = sqlx::query_as(
        "SELECT position_s, label FROM audiobook_bookmarks WHERE item_id = ? ORDER BY position_s")
        .bind(item_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(p, l)| (p, l.unwrap_or_default())).collect())
}

/// Every bookmark across a whole book (its chapters are separate items), as
/// `(item_id, position_s, label)` in chapter-then-position order.
///
/// The player panel's chips only ever show the PLAYING chapter's marks; a book
/// is a list of chapters, so the detail page needs the union or half the
/// bookmarks are invisible from the place you would look for them.
pub async fn book_bookmarks(pool: &SqlitePool, item_ids: &[i64]) -> Result<Vec<(i64, f64, String)>> {
    if item_ids.is_empty() { return Ok(vec![]); }
    let list = item_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let rows: Vec<(i64, f64, Option<String>)> = sqlx::query_as(&format!(
        "SELECT item_id, position_s, label FROM audiobook_bookmarks
         WHERE item_id IN ({list}) ORDER BY item_id, position_s")).fetch_all(pool).await?;
    // Preserve the caller's chapter order rather than the id order — chapter 10
    // can have a lower rowid than chapter 2.
    let rank: std::collections::HashMap<i64, usize> =
        item_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut out: Vec<(i64, f64, String)> = rows.into_iter()
        .map(|(id, p, l)| (id, p, l.unwrap_or_default())).collect();
    out.sort_by(|a, b| rank.get(&a.0).cmp(&rank.get(&b.0))
        .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    Ok(out)
}

/// Drop one bookmark, identified by the pair that made it.
pub async fn remove_bookmark(pool: &SqlitePool, item_id: i64, position_s: f64) -> Result<()> {
    // A float equality would be fragile across a round-trip; a one-second
    // window is far tighter than any two bookmarks a person would set.
    sqlx::query("DELETE FROM audiobook_bookmarks WHERE item_id = ? AND ABS(position_s - ?) < 1.0")
        .bind(item_id).bind(position_s).execute(pool).await?;
    Ok(())
}

/// Give a bookmark a name. An empty label clears back to the time stamp.
pub async fn rename_bookmark(pool: &SqlitePool, item_id: i64, position_s: f64, label: &str) -> Result<()> {
    sqlx::query("UPDATE audiobook_bookmarks SET label = ? WHERE item_id = ? AND ABS(position_s - ?) < 1.0")
        .bind(if label.trim().is_empty() { None } else { Some(label.trim()) })
        .bind(item_id).bind(position_s).execute(pool).await?;
    Ok(())
}

/// Mark a book finished (or un-finish it for a re-listen). Applies to every
/// chapter, because "finished" is a property of the book, not of one file.
pub async fn set_finished(pool: &SqlitePool, item_ids: &[i64], finished: bool) -> Result<()> {
    for id in item_ids {
        sqlx::query(
            "INSERT INTO audiobook_progress (item_id, position_s, finished, updated) VALUES (?, 0, ?, ?)
             ON CONFLICT(item_id) DO UPDATE SET finished = excluded.finished, updated = excluded.updated,
                 position_s = CASE WHEN excluded.finished = 0 THEN 0 ELSE audiobook_progress.position_s END")
            .bind(id).bind(if finished { 1 } else { 0 }).bind(now()).execute(pool).await?;
    }
    Ok(())
}

/// Per-book playback speed, remembered so a fast narrator stays fast and the
/// next book starts at its own pace. Stored on the book's first chapter.
pub async fn book_speed(pool: &SqlitePool, first_chapter: i64) -> Result<f64> {
    let s: Option<f64> = sqlx::query_scalar("SELECT speed FROM audiobook_progress WHERE item_id = ?")
        .bind(first_chapter).fetch_optional(pool).await?;
    Ok(clamp_speed(s.unwrap_or(1.0)))
}

pub async fn set_book_speed(pool: &SqlitePool, item_ids: &[i64], speed: f64) -> Result<()> {
    let s = clamp_speed(speed);
    for id in item_ids {
        sqlx::query(
            "INSERT INTO audiobook_progress (item_id, position_s, speed, updated) VALUES (?, 0, ?, ?)
             ON CONFLICT(item_id) DO UPDATE SET speed = excluded.speed, updated = excluded.updated")
            .bind(id).bind(s).bind(now()).execute(pool).await?;
    }
    Ok(())
}

// SQL predicate: track_meta.folder is `?1` itself OR anything under it. The
// user points the Audiobooks section at a PARENT directory ("my audiobooks")
// whose sub-folders are the individual books; per-file `folder` is the book's
// own sub-folder, so an exact `folder = ?` never matched and every chapter
// leaked into My Music as songs. Both separators so a Windows-written DB works.
const UNDER_FOLDER: &str =
    "(folder = ?1 OR folder LIKE ?1 || '/%' OR folder LIKE ?1 || '\\%')";

/// Strip trailing path separators: folder pickers hand back "/x/books/" while
/// `track_meta.folder` (from `Path::parent()`) is "/x/books" — an exact match
/// or a `?1 || '/%'` LIKE with the slash kept silently matches NOTHING, which
/// is precisely how audiobook folders leaked into My Music.
fn norm_folder(folder: &str) -> &str {
    let t = folder.trim_end_matches(['/', '\\']);
    if t.is_empty() { folder } else { t }
}

/// Set (or clear) `is_audiobook` for every track in `folder` — including its
/// sub-folders (each sub-folder stays its own book via `book_folders`).
/// Returns the number of track_meta rows updated.
pub async fn set_folder_flag(pool: &SqlitePool, folder: &str, on: bool) -> Result<u64> {
    let res = sqlx::query(&format!("UPDATE track_meta SET is_audiobook = ?2 WHERE {UNDER_FOLDER}"))
        .bind(norm_folder(folder))
        .bind(if on { 1 } else { 0 })
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Flag a folder that the user added via the **Audiobooks** section
/// (np.p5.music.audiobook-detect). Returns `(flagged, hidden)` (hidden is
/// always 0 now — the old metadata gate that hid untagged files is gone).
pub async fn flag_audiobook_folder(pool: &SqlitePool, folder: &str) -> Result<(u64, u64)> {
    let folder = norm_folder(folder);
    // The user added this folder to the AUDIOBOOKS section — that's the
    // strongest signal there is; audiobook chapters are usually plain MP3s with
    // no spoken-word tags, so a metadata gate can't be trusted here. Flag the
    // whole subtree (each sub-folder still renders as its own book via
    // `book_folders`), and un-hide anything a previous metadata-gated pass
    // wrongly buried with `missing_since`.
    let restored = sqlx::query(
        &format!("UPDATE items SET missing_since = NULL \
                  WHERE missing_since IS NOT NULL AND id IN (\
                     SELECT item_id FROM track_meta WHERE {UNDER_FOLDER})"))
        .bind(folder).execute(pool).await?.rows_affected();
    let flagged = set_folder_flag(pool, folder, true).await?;
    let _ = restored;
    Ok((flagged, 0))
}

/// Distinct audiobook folders with chapter counts, ordered by folder path.
/// One row per book card (np.p5.music.audiobook-chapters).
pub async fn book_folders(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT folder, COUNT(*) AS n FROM track_meta \
         WHERE is_audiobook = 1 AND folder IS NOT NULL AND folder <> '' \
         GROUP BY folder ORDER BY folder")
        .fetch_all(pool).await?;
    Ok(rows)
}

/// item_ids of one book's chapters, ordered by track_no then item_id.
pub async fn book_chapters(pool: &SqlitePool, folder: &str) -> Result<Vec<i64>> {
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT item_id FROM track_meta \
         WHERE is_audiobook = 1 AND folder = ? \
         ORDER BY COALESCE(track_no, 1000000), item_id")
        .bind(folder).fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn book_bookmarks_span_chapters_in_chapter_order() {
        let (_t, pool) = open_pool().await;
        let c1 = add_track(&pool, "/books/dune/ch01.mp3").await;
        let c2 = add_track(&pool, "/books/dune/ch02.mp3").await;
        add_bookmark(&pool, c2, 30.0, "second file").await.unwrap();
        add_bookmark(&pool, c1, 90.0, "later in ch1").await.unwrap();
        add_bookmark(&pool, c1, 10.0, "").await.unwrap();
        // Chapter order comes from the caller's list, not from the ids.
        let got = book_bookmarks(&pool, &[c1, c2]).await.unwrap();
        assert_eq!(got.iter().map(|(id, p, _)| (*id, *p)).collect::<Vec<_>>(),
                   vec![(c1, 10.0), (c1, 90.0), (c2, 30.0)]);
        assert_eq!(got[0].2, "", "an unnamed bookmark keeps an empty label");

        rename_bookmark(&pool, c1, 10.0, "  opening  ").await.unwrap();
        assert_eq!(book_bookmarks(&pool, &[c1, c2]).await.unwrap()[0].2, "opening");
        // Clearing the name returns it to unnamed rather than to whitespace.
        rename_bookmark(&pool, c1, 10.0, "   ").await.unwrap();
        assert_eq!(book_bookmarks(&pool, &[c1, c2]).await.unwrap()[0].2, "");

        remove_bookmark(&pool, c1, 10.4).await.unwrap();   // within the 1s window
        assert_eq!(book_bookmarks(&pool, &[c1, c2]).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn finished_and_speed_apply_to_the_whole_book() {
        let (_t, pool) = open_pool().await;
        let c1 = add_track(&pool, "/books/dune/ch01.mp3").await;
        let c2 = add_track(&pool, "/books/dune/ch02.mp3").await;
        save_progress(&pool, c1, 120.0, 1.0).await.unwrap();
        set_finished(&pool, &[c1, c2], true).await.unwrap();
        let fin: Vec<i64> = sqlx::query_scalar("SELECT finished FROM audiobook_progress ORDER BY item_id")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(fin, vec![1, 1]);
        // Un-finishing is a re-listen: position goes back to the start.
        set_finished(&pool, &[c1, c2], false).await.unwrap();
        let (pos, _) = resume(&pool, c1).await.unwrap();
        assert_eq!(pos, 0.0);

        set_book_speed(&pool, &[c1, c2], 9.0).await.unwrap();   // clamped
        assert_eq!(book_speed(&pool, c1).await.unwrap(), MAX_SPEED);
    }

    #[tokio::test]
    async fn folder_flag_scopes_to_one_folder() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/music/song.mp3").await;
        for (id, folder) in [(a, "/books/dune"), (b, "/books/dune"), (c, "/music")] {
            sqlx::query("UPDATE track_meta SET folder = ? WHERE item_id = ?")
                .bind(folder).bind(id).execute(&pool).await.unwrap();
        }
        let n = set_folder_flag(&pool, "/books/dune", true).await.unwrap();
        assert_eq!(n, 2);
        let flagged: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE is_audiobook = 1 ORDER BY item_id")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(flagged, vec![a, b]);
        let n2 = set_folder_flag(&pool, "/books/dune", false).await.unwrap();
        assert_eq!(n2, 2);
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(still, 0);
    }

    #[tokio::test]
    async fn book_folders_groups_and_counts() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/books/hobbit/all.m4b").await;
        for (id, folder) in [(a, "/books/dune"), (b, "/books/dune"), (c, "/books/hobbit")] {
            sqlx::query("UPDATE track_meta SET folder = ?, is_audiobook = 1 WHERE item_id = ?")
                .bind(folder).bind(id).execute(&pool).await.unwrap();
        }
        let books = book_folders(&pool).await.unwrap();
        assert_eq!(books, vec![
            ("/books/dune".to_string(), 2),
            ("/books/hobbit".to_string(), 1),
        ]);
        let dune = book_chapters(&pool, "/books/dune").await.unwrap();
        assert_eq!(dune, vec![a, b]);
    }

    #[test]
    fn speed_clamps_and_keeps_pitch() {
        assert_eq!(clamp_speed(9.0), 3.0);
        assert_eq!(clamp_speed(0.1), 0.5);
        let o = mpv_speed_options(1.5);
        assert!(o.contains(&"--audio-pitch-correction=yes".to_string()));
        assert!(o.contains(&"--speed=1.5".to_string()));
    }

    #[tokio::test]
    async fn progress_roundtrip() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/book.m4b").await;
        assert_eq!(resume(&pool, a).await.unwrap(), (0.0, 1.0));
        save_progress(&pool, a, 1234.5, 1.25).await.unwrap();
        let (pos, sp) = resume(&pool, a).await.unwrap();
        assert!((pos - 1234.5).abs() < 1e-6);
        assert!((sp - 1.25).abs() < 1e-6);
    }
}
