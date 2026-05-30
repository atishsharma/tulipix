//! `np.p4.search.ocr` — tesseract-rs OCR → `ocr_text` FTS table.
//!
//! Tesseract extracts text from PDFs + images in the Tools worker; the text
//! lands in an FTS5 table so universal search can match document contents.
//! This owns the FTS schema, upsert, and the MATCH query (returning item ids
//! ranked by bm25).

use anyhow::Result;
use sqlx::SqlitePool;

pub const OCR_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS ocr_text USING fts5(
    item_id UNINDEXED,
    section UNINDEXED,
    content,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(OCR_SCHEMA).execute(pool).await?;
    Ok(())
}

/// Replace any existing OCR text for an item, then insert the fresh extract.
pub async fn upsert(pool: &SqlitePool, item_id: i64, section: &str, content: &str) -> Result<()> {
    sqlx::query("DELETE FROM ocr_text WHERE item_id = ? AND section = ?")
        .bind(item_id).bind(section).execute(pool).await?;
    sqlx::query("INSERT INTO ocr_text (item_id, section, content) VALUES (?,?,?)")
        .bind(item_id).bind(section).bind(content).execute(pool).await?;
    Ok(())
}

/// FTS MATCH search → (item_id, section) ranked best-first.
pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT item_id, section FROM ocr_text WHERE ocr_text MATCH ? ORDER BY bm25(ocr_text) LIMIT ?",
    ).bind(query).bind(limit).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ocr.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "search".into(), path, url };
        let p = h.pool().await.unwrap();
        apply(&p).await.unwrap();
        (tmp, p)
    }

    #[tokio::test]
    async fn index_and_match() {
        let (_t, p) = pool().await;
        upsert(&p, 1, "photos", "invoice total amount due").await.unwrap();
        upsert(&p, 2, "books", "the quick brown fox").await.unwrap();
        let r = search(&p, "invoice", 10).await.unwrap();
        assert_eq!(r, vec![(1, "photos".to_string())]);
        // re-upsert replaces, no duplicate hit
        upsert(&p, 1, "photos", "receipt grocery").await.unwrap();
        assert!(search(&p, "invoice", 10).await.unwrap().is_empty());
        assert_eq!(search(&p, "grocery", 10).await.unwrap().len(), 1);
    }
}
