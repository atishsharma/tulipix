//! `np.p4.books.reader.navigation` — TOC sidebar, chapter scrubbing, comic
//! thumbnail timeline.
//!
//! Persists the table of contents and answers the two questions the nav UI
//! asks every frame: "which chapter is page N in?" (for the scrubber label)
//! and "what page does chapter C start at?" (for jump). The thumbnail timeline
//! just maps a 0..1 scrub position to a page index.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq)]
pub struct TocEntry {
    pub idx: i64,
    pub title: String,
    pub page: Option<i64>,
}

/// Replace a book's TOC with `entries` (in order).
pub async fn set_toc(pool: &SqlitePool, item_id: i64, entries: &[(String, Option<i64>)]) -> Result<()> {
    sqlx::query("DELETE FROM toc WHERE item_id = ?").bind(item_id).execute(pool).await?;
    for (i, (title, page)) in entries.iter().enumerate() {
        sqlx::query("INSERT INTO toc (item_id, idx, title, page) VALUES (?,?,?,?)")
            .bind(item_id).bind(i as i64).bind(title).bind(page).execute(pool).await?;
    }
    Ok(())
}

pub async fn toc(pool: &SqlitePool, item_id: i64) -> Result<Vec<TocEntry>> {
    let rows: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT idx, title, page FROM toc WHERE item_id = ? ORDER BY idx",
    ).bind(item_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(idx, title, page)| TocEntry { idx, title, page }).collect())
}

/// The TOC entry covering `page` — the last entry whose start page ≤ page.
pub fn chapter_for_page(entries: &[TocEntry], page: i64) -> Option<&TocEntry> {
    entries.iter().filter(|e| e.page.map_or(false, |p| p <= page)).last()
}

/// Map a 0..1 scrub position to a page index for `total` pages.
pub fn scrub_to_page(fraction: f64, total: usize) -> usize {
    if total == 0 { return 0; }
    ((fraction.clamp(0.0, 1.0) * (total - 1) as f64).round() as usize).min(total - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_book};

    #[tokio::test]
    async fn toc_roundtrip_and_chapter_lookup() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        set_toc(&pool, id, &[("Intro".into(), Some(0)), ("Ch1".into(), Some(10)), ("Ch2".into(), Some(40))]).await.unwrap();
        let t = toc(&pool, id).await.unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(chapter_for_page(&t, 25).unwrap().title, "Ch1");
        assert_eq!(chapter_for_page(&t, 0).unwrap().title, "Intro");
        // re-set replaces, not appends
        set_toc(&pool, id, &[("Only".into(), Some(0))]).await.unwrap();
        assert_eq!(toc(&pool, id).await.unwrap().len(), 1);
    }

    #[test]
    fn scrub_maps_extremes() {
        assert_eq!(scrub_to_page(0.0, 100), 0);
        assert_eq!(scrub_to_page(1.0, 100), 99);
        assert_eq!(scrub_to_page(0.5, 101), 50);
        assert_eq!(scrub_to_page(0.5, 0), 0);
    }
}
