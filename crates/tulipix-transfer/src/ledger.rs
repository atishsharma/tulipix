//! Recent Transfers, persisted. A transfer is an event, not a file, so this
//! does not use the shared proxy schema.

use anyhow::Result;
use sqlx::SqlitePool;

/// Rows per page in the Recent Transfers list. The pagination test derives its
/// expectations from this rather than repeating a literal.
pub const PAGE_SIZE: usize = 10;

/// Which column Recent Transfers is ordered by.
///
/// An enum rather than a column name passed in from the UI: this ends up
/// interpolated into SQL — `ORDER BY` cannot take a bound parameter — so the
/// set of legal values has to be closed at compile time. The page offset is
/// still bound normally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    Name,
    Size,
    Direction,
    Status,
    Peer,
    #[default]
    Time,
}

impl Sort {
    /// Maps the table's visual column order, which the header cells send as an
    /// index. Anything unexpected falls back to the default rather than
    /// erroring — a bad index is a UI bug, not something to fail a query over.
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Sort::Name,
            1 => Sort::Size,
            2 => Sort::Direction,
            3 => Sort::Status,
            4 => Sort::Peer,
            _ => Sort::Time,
        }
    }

    fn column(self) -> &'static str {
        match self {
            // Case-insensitive, or "Zebra.pdf" sorts before "apple.pdf".
            Sort::Name => "name COLLATE NOCASE",
            Sort::Size => "bytes",
            Sort::Direction => "direction",
            Sort::Status => "status",
            Sort::Peer => "peer",
            Sort::Time => "at",
        }
    }
}

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS transfers (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    direction TEXT    NOT NULL,
    name      TEXT    NOT NULL,
    abs_path  TEXT,
    bytes     INTEGER NOT NULL,
    peer      TEXT    NOT NULL,
    status    TEXT    NOT NULL,
    at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS transfers_at_idx ON transfers(at DESC);

CREATE TABLE IF NOT EXISTS devices (
    token     TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    issued    INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    expires   INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Opens `transfers.db` next to the section databases, inheriting WAL mode and
/// the pool settings from the shared handle.
pub async fn open() -> Result<SqlitePool> {
    let handle = tulipix_core::db::DbHandle::open("transfers")?;
    let pool = handle.pool().await?;
    apply_schema(&pool).await?;
    Ok(pool)
}

#[derive(Clone, Debug)]
pub struct Row {
    pub direction: &'static str,
    pub name: String,
    pub abs_path: Option<String>,
    pub bytes: i64,
    pub peer: String,
    pub status: &'static str,
}

impl Row {
    pub fn sent(name: &str, bytes: i64, peer: &str) -> Self {
        Self {
            direction: "out",
            name: name.into(),
            abs_path: None,
            bytes,
            peer: peer.into(),
            status: "ok",
        }
    }

    pub fn received(name: &str, path: &str, bytes: i64, peer: &str) -> Self {
        Self {
            direction: "in",
            name: name.into(),
            abs_path: Some(path.into()),
            bytes,
            peer: peer.into(),
            status: "ok",
        }
    }

    pub fn failed(mut self) -> Self {
        self.status = "failed";
        self
    }
}

pub async fn record(pool: &SqlitePool, row: Row, at: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO transfers (direction, name, abs_path, bytes, peer, status, at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(row.direction)
    .bind(&row.name)
    .bind(&row.abs_path)
    .bind(row.bytes)
    .bind(&row.peer)
    .bind(row.status)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

pub struct Entry {
    pub name: String,
    pub abs_path: Option<String>,
    pub direction: String,
    pub bytes: i64,
    pub peer: String,
    pub status: String,
    pub at: i64,
}

/// One page of the ledger, ordered by `sort`.
///
/// Ordering happens here rather than on the loaded page: with ten rows to a
/// page, sorting the slice the UI already holds would only shuffle those ten
/// and leave the other pages untouched.
pub async fn recent(
    pool: &SqlitePool,
    page: usize,
    sort: Sort,
    desc: bool,
) -> Result<Vec<Entry>> {
    // Both halves come from closed sets chosen right here, never from a string
    // the UI supplied. `id DESC` breaks ties so equal keys keep a stable order
    // across pages instead of drifting between queries.
    let column = sort.column();
    let dir = if desc { "DESC" } else { "ASC" };
    let rows = sqlx::query_as::<_, (String, Option<String>, String, i64, String, String, i64)>(
        &format!(
            "SELECT direction, abs_path, name, bytes, peer, status, at
             FROM transfers ORDER BY {column} {dir}, id DESC LIMIT ? OFFSET ?"
        ),
    )
    .bind(PAGE_SIZE as i64)
    .bind((page * PAGE_SIZE) as i64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(direction, abs_path, name, bytes, peer, status, at)| Entry {
            direction,
            abs_path,
            name,
            bytes,
            peer,
            status,
            at,
        })
        .collect())
}

pub async fn count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transfers").fetch_one(pool).await?)
}

pub async fn clear_transfers(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM transfers").execute(pool).await?;
    Ok(())
}

pub async fn remember_device(
    pool: &SqlitePool,
    token: &str,
    label: &str,
    now: i64,
    expires: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO devices (token, label, issued, last_seen, expires) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(token) DO UPDATE SET last_seen = excluded.last_seen",
    )
    .bind(token)
    .bind(label)
    .bind(now)
    .bind(now)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(())
}

/// Only devices whose token has not expired. Expired rows are left in place for
/// `forget_expired` to sweep, so a phone that reappears at hour 49 is told to
/// re-pair rather than being silently unknown.
pub async fn devices(pool: &SqlitePool, now: i64) -> Result<Vec<(String, String, i64, i64)>> {
    let rows = sqlx::query_as::<_, (String, String, i64, i64)>(
        "SELECT token, label, last_seen, expires FROM devices WHERE expires > ? ORDER BY last_seen DESC",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn forget_device(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM devices WHERE token = ?").bind(token).execute(pool).await?;
    Ok(())
}

pub async fn forget_expired(pool: &SqlitePool, now: i64) -> Result<()> {
    sqlx::query("DELETE FROM devices WHERE expires <= ?").bind(now).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        apply_schema(&pool).await.unwrap();
        pool
    }

    /// The default view: newest first.
    async fn newest(pool: &sqlx::SqlitePool, page: usize) -> Vec<Entry> {
        recent(pool, page, Sort::default(), true).await.unwrap()
    }

    #[tokio::test]
    async fn rows_come_back_newest_first() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("old.mp3", 10, "1.2.3.4"), 100).await.unwrap();
        record(&pool, Row::sent("new.mp3", 20, "1.2.3.4"), 200).await.unwrap();

        let page = newest(&pool, 0).await;
        assert_eq!(
            page.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["new.mp3", "old.mp3"]
        );
    }

    #[tokio::test]
    async fn pagination_is_derived_from_the_page_size_constant() {
        let pool = mem_pool().await;
        for n in 0..(PAGE_SIZE + 5) {
            record(&pool, Row::sent(&format!("f{n}"), 1, "1.2.3.4"), 100 + n as i64).await.unwrap();
        }
        assert_eq!(newest(&pool, 0).await.len(), PAGE_SIZE);
        assert_eq!(newest(&pool, 1).await.len(), 5);
    }

    #[tokio::test]
    async fn every_column_sorts_both_ways() {
        let pool = mem_pool().await;
        record(&pool, Row::received("beta.mp3", "/in/b", 300, "10.0.0.9"), 100).await.unwrap();
        record(&pool, Row::sent("alpha.mp3", 100, "10.0.0.2"), 300).await.unwrap();
        record(&pool, Row::sent("gamma.mp3", 200, "10.0.0.5").failed(), 200).await.unwrap();

        async fn names(pool: &sqlx::SqlitePool, sort: Sort, desc: bool) -> Vec<String> {
            recent(pool, 0, sort, desc).await.unwrap().into_iter().map(|e| e.name).collect()
        }

        assert_eq!(names(&pool, Sort::Name, false).await, ["alpha.mp3", "beta.mp3", "gamma.mp3"]);
        assert_eq!(names(&pool, Sort::Name, true).await, ["gamma.mp3", "beta.mp3", "alpha.mp3"]);
        // 100 / 200 / 300 — a numeric order, not the lexical one a TEXT column
        // would have given ("100" < "200" < "300" happens to agree here, so the
        // ascending case is checked against a size that would break it).
        assert_eq!(names(&pool, Sort::Size, false).await, ["alpha.mp3", "gamma.mp3", "beta.mp3"]);
        assert_eq!(names(&pool, Sort::Size, true).await, ["beta.mp3", "gamma.mp3", "alpha.mp3"]);
        // "in" sorts before "out".
        assert_eq!(names(&pool, Sort::Direction, false).await[0], "beta.mp3");
        // "failed" sorts before "ok".
        assert_eq!(names(&pool, Sort::Status, false).await[0], "gamma.mp3");
        assert_eq!(names(&pool, Sort::Peer, false).await[0], "alpha.mp3");
        assert_eq!(names(&pool, Sort::Time, true).await[0], "alpha.mp3");
    }

    #[tokio::test]
    async fn sizes_sort_numerically_not_lexically() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("small", 9, "1.2.3.4"), 100).await.unwrap();
        record(&pool, Row::sent("big", 1000, "1.2.3.4"), 200).await.unwrap();

        // Lexically "1000" < "9", so this is the case that catches a TEXT
        // column or a string comparison sneaking in.
        let asc = recent(&pool, 0, Sort::Size, false).await.unwrap();
        assert_eq!(asc[0].name, "small");
    }

    #[tokio::test]
    async fn the_column_index_the_ui_sends_maps_to_the_visual_order() {
        // Name | Size | Type | Status | From/To | Time — the header's own order.
        assert_eq!(Sort::from_index(0), Sort::Name);
        assert_eq!(Sort::from_index(1), Sort::Size);
        assert_eq!(Sort::from_index(2), Sort::Direction);
        assert_eq!(Sort::from_index(3), Sort::Status);
        assert_eq!(Sort::from_index(4), Sort::Peer);
        assert_eq!(Sort::from_index(5), Sort::Time);
        // Out of range falls back rather than panicking.
        assert_eq!(Sort::from_index(99), Sort::Time);
        assert_eq!(Sort::from_index(-1), Sort::Time);
    }

    #[tokio::test]
    async fn clearing_transfers_leaves_paired_devices_alone() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("a", 1, "1.2.3.4"), 100).await.unwrap();
        remember_device(&pool, "tok", "Android · Chrome", 100, 100 + 48 * 3600).await.unwrap();

        clear_transfers(&pool).await.unwrap();

        assert!(newest(&pool, 0).await.is_empty());
        assert_eq!(devices(&pool, 200).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_expired_device_is_not_listed() {
        let pool = mem_pool().await;
        remember_device(&pool, "tok", "Android · Chrome", 100, 100 + 48 * 3600).await.unwrap();
        assert_eq!(devices(&pool, 100 + 47 * 3600).await.unwrap().len(), 1);
        assert_eq!(devices(&pool, 100 + 49 * 3600).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_received_row_keeps_the_path_it_landed_at() {
        let pool = mem_pool().await;
        record(&pool, Row::received("a.mp3", "/inbox/a.mp3", 5, "1.2.3.4"), 100).await.unwrap();
        let page = newest(&pool, 0).await;
        assert_eq!(page[0].direction, "in");
        assert_eq!(page[0].abs_path.as_deref(), Some("/inbox/a.mp3"));
    }

    #[tokio::test]
    async fn a_failed_transfer_is_recorded_as_failed() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("x", 1, "1.2.3.4").failed(), 100).await.unwrap();
        assert_eq!(newest(&pool, 0).await[0].status, "failed");
    }
}
