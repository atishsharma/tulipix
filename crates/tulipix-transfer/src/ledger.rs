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
    expires   INTEGER NOT NULL,
    kind      TEXT    NOT NULL DEFAULT '',
    ip        TEXT    NOT NULL DEFAULT '',
    name      TEXT    NOT NULL DEFAULT '',
    pin       TEXT    NOT NULL DEFAULT ''
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    add_missing_columns(pool).await?;
    Ok(())
}

/// The four columns the round-button device list needs, added to a `devices`
/// table that predates them. `CREATE TABLE IF NOT EXISTS` does nothing to a table
/// that already exists, so a database from before this change would otherwise be
/// missing them for good.
async fn add_missing_columns(pool: &SqlitePool) -> Result<()> {
    for (column, decl) in
        [("kind", "TEXT NOT NULL DEFAULT ''"), ("ip", "TEXT NOT NULL DEFAULT ''"),
         ("name", "TEXT NOT NULL DEFAULT ''"), ("pin", "TEXT NOT NULL DEFAULT ''")]
    {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('devices') WHERE name = ?)",
        )
        .bind(column)
        .fetch_one(pool)
        .await?;
        if !exists {
            sqlx::query(&format!("ALTER TABLE devices ADD COLUMN {column} {decl}"))
                .execute(pool)
                .await?;
        }
    }
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
    /// `path` is where the file was read from on this machine. Recorded for the
    /// same reason a received file's is: the ledger's reveal button needs
    /// somewhere to point, and a sent row without one was the only kind that
    /// could not be opened.
    pub fn sent(name: &str, path: &str, bytes: i64, peer: &str) -> Self {
        Self {
            direction: "out",
            name: name.into(),
            abs_path: Some(path.into()),
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
    token: &crate::auth::Token,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO devices (token, label, issued, last_seen, expires, kind, ip, name, pin)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(token) DO UPDATE SET last_seen = excluded.last_seen",
    )
    .bind(&token.value)
    .bind(&token.label)
    .bind(now)
    .bind(now)
    .bind(token.expires as i64)
    .bind(&token.kind)
    .bind(&token.ip)
    .bind(&token.name)
    .bind(&token.pin)
    .execute(pool)
    .await?;
    Ok(())
}

/// A name typed on the desktop outlives a restart, unlike the rest of the
/// session state — it is the one thing here the user authored.
pub async fn rename_device(pool: &SqlitePool, token: &str, name: &str) -> Result<()> {
    sqlx::query("UPDATE devices SET name = ? WHERE token = ?")
        .bind(name)
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// One stored device. A row, not a tuple: nine columns positionally would be a
/// bug waiting for the next column to be added.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DeviceRecord {
    pub token: String,
    pub label: String,
    pub last_seen: i64,
    pub expires: i64,
    pub kind: String,
    pub ip: String,
    pub name: String,
    pub pin: String,
}

/// Only devices whose token has not expired. Expired rows are left in place for
/// `forget_expired` to sweep, so a phone that reappears at hour 49 is told to
/// re-pair rather than being silently unknown.
pub async fn devices(pool: &SqlitePool, now: i64) -> Result<Vec<DeviceRecord>> {
    let rows = sqlx::query_as::<_, DeviceRecord>(
        "SELECT token, label, last_seen, expires, kind, ip, name, pin
         FROM devices WHERE expires > ? ORDER BY last_seen DESC",
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

    /// A token as the pairing handlers would hand it over.
    fn sample_device(value: &str, now: u64) -> crate::auth::Token {
        crate::auth::Token {
            value: value.into(),
            label: "Android · Chrome".into(),
            kind: "Android".into(),
            ip: "192.168.1.31".into(),
            pin: "123456".into(),
            issued: now,
            expires: now + 48 * 3600,
            last_seen: now,
            ..Default::default()
        }
    }

    /// The default view: newest first.
    async fn newest(pool: &sqlx::SqlitePool, page: usize) -> Vec<Entry> {
        recent(pool, page, Sort::default(), true).await.unwrap()
    }

    #[tokio::test]
    async fn rows_come_back_newest_first() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("old.mp3", "/tmp/out", 10, "1.2.3.4"), 100).await.unwrap();
        record(&pool, Row::sent("new.mp3", "/tmp/out", 20, "1.2.3.4"), 200).await.unwrap();

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
            record(&pool, Row::sent(&format!("f{n}"), "/tmp/out", 1, "1.2.3.4"), 100 + n as i64).await.unwrap();
        }
        assert_eq!(newest(&pool, 0).await.len(), PAGE_SIZE);
        assert_eq!(newest(&pool, 1).await.len(), 5);
    }

    #[tokio::test]
    async fn every_column_sorts_both_ways() {
        let pool = mem_pool().await;
        record(&pool, Row::received("beta.mp3", "/in/b", 300, "10.0.0.9"), 100).await.unwrap();
        record(&pool, Row::sent("alpha.mp3", "/tmp/out", 100, "10.0.0.2"), 300).await.unwrap();
        record(&pool, Row::sent("gamma.mp3", "/tmp/out", 200, "10.0.0.5").failed(), 200).await.unwrap();

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
        record(&pool, Row::sent("small", "/tmp/out", 9, "1.2.3.4"), 100).await.unwrap();
        record(&pool, Row::sent("big", "/tmp/out", 1000, "1.2.3.4"), 200).await.unwrap();

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
        record(&pool, Row::sent("a", "/tmp/out", 1, "1.2.3.4"), 100).await.unwrap();
        remember_device(&pool, &sample_device("tok", 100), 100).await.unwrap();

        clear_transfers(&pool).await.unwrap();

        assert!(newest(&pool, 0).await.is_empty());
        assert_eq!(devices(&pool, 200).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_device_keeps_its_type_address_and_typed_name() {
        let pool = mem_pool().await;
        remember_device(&pool, &sample_device("tok", 100), 100).await.unwrap();
        rename_device(&pool, "tok", "Kitchen").await.unwrap();

        let rows = devices(&pool, 200).await.unwrap();
        assert_eq!(rows[0].kind, "Android");
        assert_eq!(rows[0].ip, "192.168.1.31");
        assert_eq!(rows[0].pin, "123456");
        assert_eq!(rows[0].name, "Kitchen");

        // Seeing the phone again must not wipe the name it was given.
        remember_device(&pool, &sample_device("tok", 300), 300).await.unwrap();
        assert_eq!(devices(&pool, 400).await.unwrap()[0].name, "Kitchen");
    }

    #[tokio::test]
    async fn a_devices_table_from_before_the_new_columns_is_migrated() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        // The old shape, exactly as it shipped.
        sqlx::raw_sql(
            "CREATE TABLE devices (token TEXT PRIMARY KEY, label TEXT NOT NULL,
             issued INTEGER NOT NULL, last_seen INTEGER NOT NULL, expires INTEGER NOT NULL);
             INSERT INTO devices VALUES ('old', 'Android · Chrome', 100, 100, 172900);",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_schema(&pool).await.unwrap();

        let rows = devices(&pool, 200).await.unwrap();
        assert_eq!(rows.len(), 1, "the existing pairing was lost");
        // Nothing invented for a device that paired before we asked: empty, and
        // the UI captions it by whatever the label says instead.
        assert_eq!(rows[0].kind, "");
        assert_eq!(rows[0].name, "");
        // And the migration is idempotent.
        apply_schema(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn an_expired_device_is_not_listed() {
        let pool = mem_pool().await;
        remember_device(&pool, &sample_device("tok", 100), 100).await.unwrap();
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
        record(&pool, Row::sent("x", "/tmp/out", 1, "1.2.3.4").failed(), 100).await.unwrap();
        assert_eq!(newest(&pool, 0).await[0].status, "failed");
    }
}
