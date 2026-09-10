//! `np.p4.music.playlists` — manual + smart playlists.
//!
//! Manual playlists are an ordered `playlist_items` list (append + reorder).
//! Smart playlists store a JSON rule set that compiles to a parameterized
//! `track_meta` WHERE clause, so they re-evaluate live as the library changes.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Field { Genre, Year, Rating, Loved, PlayCount, Bpm, Artist, MusicKey, DrScore, Added }

impl Field {
    /// The id this field serialises as, which is also what an editor sends back.
    pub fn id(&self) -> &'static str {
        Field::all().iter().find(|(f, _, _)| f == self).map(|(_, i, _)| *i).unwrap_or("genre")
    }

    /// The SQL the condition compares against. Unqualified names resolve to
    /// `track_meta`; `Added` and `Artist` reach past it, so they say where.
    fn column(&self) -> &'static str {
        match self {
            Field::Genre => "genre", Field::Year => "year", Field::Rating => "rating",
            Field::Loved => "loved", Field::PlayCount => "play_count", Field::Bpm => "bpm",
            Field::MusicKey => "music_key", Field::DrScore => "dr_score",
            Field::Artist => "(SELECT name FROM artists WHERE artists.id = track_meta.artist_id)",
            // Days since it was added, so "added less than 90" reads as
            // "added within the last 90 days" and needs no special operator.
            // Evaluated per row against the clock, so a saved rule keeps
            // meaning the same thing tomorrow.
            Field::Added => "((CAST(strftime('%s','now') AS INTEGER) - items.added) / 86400)",
        }
    }

    /// What kind of input the value is, so an editor can offer the right one
    /// rather than a text box for everything.
    pub fn kind(&self) -> &'static str {
        match self {
            Field::Genre | Field::Artist | Field::MusicKey => "text",
            Field::Loved => "bool",
            Field::Added => "days",
            _ => "number",
        }
    }

    /// The field an editor's dropdown named, or `None` for an id from a build
    /// that had a field this one does not.
    pub fn from_id(id: &str) -> Option<Field> {
        Field::all().iter().find(|(_, i, _)| *i == id).map(|(f, _, _)| f.clone())
    }

    /// Every field an editor can offer, with the id its serde form uses.
    pub fn all() -> &'static [(Field, &'static str, &'static str)] {
        &[
            (Field::Genre, "genre", "Genre"),
            (Field::Artist, "artist", "Artist"),
            (Field::Year, "year", "Year"),
            (Field::Rating, "rating", "Rating"),
            (Field::Loved, "loved", "Loved"),
            (Field::PlayCount, "play_count", "Plays"),
            (Field::Bpm, "bpm", "Tempo (BPM)"),
            (Field::MusicKey, "music_key", "Key"),
            (Field::DrScore, "dr_score", "Dynamic range"),
            (Field::Added, "added", "Added (days ago)"),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Op { Eq, Ne, Gt, Lt, Gte, Lte, Contains }

impl Op {
    pub fn id(&self) -> &'static str {
        Op::all().iter().find(|(o, _, _)| o == self).map(|(_, i, _)| *i).unwrap_or("eq")
    }

    pub fn from_id(id: &str) -> Option<Op> {
        Op::all().iter().find(|(_, i, _)| *i == id).map(|(o, _, _)| o.clone())
    }

    /// Every operator an editor can offer, with the id its serde form uses.
    pub fn all() -> &'static [(Op, &'static str, &'static str)] {
        &[
            (Op::Eq, "eq", "is"),
            (Op::Ne, "ne", "is not"),
            (Op::Contains, "contains", "contains"),
            (Op::Gt, "gt", "is more than"),
            (Op::Lt, "lt", "is less than"),
            (Op::Gte, "gte", "is at least"),
            (Op::Lte, "lte", "is at most"),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition { pub field: Field, pub op: Op, pub value: String }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Combine { All, Any }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartRule {
    pub combine: Combine,
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Resolve a smart rule to matching item ids, newest first. Audiobook chapters
/// never qualify — smart playlists are a music feature (np.p5.music.audiobook-chapters).
pub async fn evaluate(pool: &SqlitePool, rule: &SmartRule) -> Result<Vec<i64>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id \
         WHERE items.missing_since IS NULL AND COALESCE(track_meta.is_audiobook, 0) = 0",
    );
    if !rule.conditions.is_empty() {
        let joiner = match rule.combine { Combine::All => " AND ", Combine::Any => " OR " };
        qb.push(" AND (");
        for (i, c) in rule.conditions.iter().enumerate() {
            if i > 0 { qb.push(joiner); }
            let col = c.field.column();
            match c.op {
                Op::Contains => { qb.push(format!("{col} LIKE ")); qb.push_bind(format!("%{}%", c.value)); }
                _ => {
                    let sym = match c.op { Op::Eq=>"=", Op::Ne=>"!=", Op::Gt=>">", Op::Lt=>"<", Op::Gte=>">=", Op::Lte=>"<=", Op::Contains=>"LIKE" };
                    qb.push(format!("{col} {sym} "));
                    // numeric fields bind as i64 where parseable, else as text
                    if let Ok(n) = c.value.parse::<i64>() { qb.push_bind(n); } else { qb.push_bind(c.value.clone()); }
                }
            }
        }
        qb.push(")");
    }
    qb.push(" ORDER BY items.added DESC");
    if let Some(l) = rule.limit { qb.push(" LIMIT "); qb.push_bind(l); }
    let rows: Vec<(i64,)> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn create(pool: &SqlitePool, name: &str, rule: Option<&SmartRule>) -> Result<i64> {
    let (is_smart, rule_json) = match rule {
        Some(r) => (1i64, Some(serde_json::to_string(r)?)),
        None => (0, None),
    };
    let t = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO playlists (name, is_smart, rule_json, created, updated) VALUES (?,?,?,?,?) RETURNING id",
    ).bind(name).bind(is_smart).bind(rule_json).bind(t).bind(t).fetch_one(pool).await?)
}

/// Rewrite a smart playlist's name and rule in place.
///
/// In place rather than delete-and-recreate: the id is what
/// `playlists.detail_id` and any open page are holding, and recreating would
/// close the page the user is editing from.
pub async fn update_smart(pool: &SqlitePool, playlist_id: i64, name: &str, rule: &SmartRule) -> Result<()> {
    sqlx::query("UPDATE playlists SET name = ?, is_smart = 1, rule_json = ?, updated = ? WHERE id = ?")
        .bind(name).bind(serde_json::to_string(rule)?).bind(now()).bind(playlist_id)
        .execute(pool).await?;
    Ok(())
}

/// The rule behind a smart playlist, or `None` when it is a manual one.
pub async fn rule_of(pool: &SqlitePool, playlist_id: i64) -> Result<Option<SmartRule>> {
    let row: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT is_smart, rule_json FROM playlists WHERE id = ?")
            .bind(playlist_id).fetch_optional(pool).await?;
    let Some((1, Some(json))) = row else { return Ok(None) };
    // A rule saved by an older build may not parse against today's Field set.
    // That is a rule to re-author, not an error to fail the page over.
    Ok(serde_json::from_str(&json).ok())
}

/// Playlist id by exact name — lets one-click smart playlists dedupe instead
/// of stacking copies.
pub async fn find_by_name(pool: &SqlitePool, name: &str) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM playlists WHERE name = ? LIMIT 1")
        .bind(name).fetch_optional(pool).await?)
}

/// Delete a playlist and its track list. Library items are untouched.
pub async fn delete(pool: &SqlitePool, playlist_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?").bind(playlist_id).execute(pool).await?;
    sqlx::query("DELETE FROM playlists WHERE id = ?").bind(playlist_id).execute(pool).await?;
    Ok(())
}

/// Append a track to a manual playlist at the next position.
pub async fn append(pool: &SqlitePool, playlist_id: i64, item_id: i64) -> Result<()> {
    let next: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position)+1, 0) FROM playlist_items WHERE playlist_id = ?")
        .bind(playlist_id).fetch_one(pool).await?;
    sqlx::query("INSERT INTO playlist_items (playlist_id, item_id, position) VALUES (?,?,?)")
        .bind(playlist_id).bind(item_id).bind(next).execute(pool).await?;
    Ok(())
}

pub async fn items(pool: &SqlitePool, playlist_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT item_id FROM playlist_items WHERE playlist_id = ? ORDER BY position")
        .bind(playlist_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Parse an M3U/M3U8 document into entry paths (np.p5.music.import-playlists).
/// `#`-prefixed directives are skipped; relative entries resolve against
/// `base_dir` (the playlist file's directory).
pub fn parse_m3u(content: &str, base_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    content.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let p = std::path::Path::new(l);
            if p.is_absolute() { p.to_path_buf() } else { base_dir.join(p) }
        })
        .collect()
}

/// Render an `#EXTM3U` document from absolute track paths.
pub fn write_m3u(paths: &[std::path::PathBuf]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for p in paths { out.push_str(&p.to_string_lossy()); out.push('\n'); }
    out
}

/// Parse a PLS document (`[playlist]` with `FileN=` keys) into entry paths
/// (np.p5.music.import-playlists). Entries keep their `FileN` order; relative
/// paths resolve against `base_dir`.
pub fn parse_pls(content: &str, base_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for line in content.lines().map(str::trim) {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("file") {
            // `fileN=value` — split the original line on '=' to keep value case.
            if let Some(eq) = line.find('=') {
                let n: u32 = rest[..rest.find('=').unwrap_or(rest.len())].parse().unwrap_or(u32::MAX);
                let val = line[eq + 1..].trim();
                if val.is_empty() { continue; }
                let p = std::path::Path::new(val);
                // Skip remote URLs; only resolve local file entries.
                if val.contains("://") && !val.starts_with("file://") { continue; }
                let path = if p.is_absolute() { p.to_path_buf() } else { base_dir.join(p) };
                entries.push((n, path));
            }
        }
    }
    entries.sort_by_key(|(n, _)| *n);
    entries.into_iter().map(|(_, p)| p).collect()
}

/// Reorder a track within a manual playlist: move the item currently at
/// `from` to index `to`, renumbering positions contiguously
/// (np.p5.music.playlists-builder in-playlist reorder).
pub async fn reorder(pool: &SqlitePool, playlist_id: i64, from: usize, to: usize) -> Result<()> {
    let mut ids = items(pool, playlist_id).await?;
    if from >= ids.len() { return Ok(()); }
    let to = to.min(ids.len().saturating_sub(1));
    let id = ids.remove(from);
    ids.insert(to, id);
    // Rewrite positions in one transaction (clear then re-insert in order).
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?").bind(playlist_id).execute(&mut *tx).await?;
    for (pos, item_id) in ids.iter().enumerate() {
        sqlx::query("INSERT INTO playlist_items (playlist_id, item_id, position) VALUES (?,?,?)")
            .bind(playlist_id).bind(item_id).bind(pos as i64).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn added_within_days_and_the_artist_reach_past_track_meta() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO artists (id, name) VALUES (1, 'Aphex Twin')")
            .execute(&pool).await.unwrap();
        let fresh = add_track(&pool, "/m/fresh.flac").await;
        let old = add_track(&pool, "/m/old.flac").await;
        let n = now();
        sqlx::query("UPDATE items SET added = ? WHERE id = ?")
            .bind(n - 10 * 86_400).bind(fresh).execute(&pool).await.unwrap();
        sqlx::query("UPDATE items SET added = ? WHERE id = ?")
            .bind(n - 400 * 86_400).bind(old).execute(&pool).await.unwrap();
        for id in [fresh, old] {
            sqlx::query("UPDATE track_meta SET artist_id = 1 WHERE item_id = ?")
                .bind(id).execute(&pool).await.unwrap();
        }

        let within_90 = SmartRule {
            combine: Combine::All,
            conditions: vec![Condition { field: Field::Added, op: Op::Lt, value: "90".into() }],
            limit: None,
        };
        assert_eq!(evaluate(&pool, &within_90).await.unwrap(), vec![fresh]);

        let by_artist = SmartRule {
            combine: Combine::All,
            conditions: vec![Condition { field: Field::Artist, op: Op::Contains, value: "aphex".into() }],
            limit: None,
        };
        assert_eq!(evaluate(&pool, &by_artist).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_smart_rule_round_trips_through_the_row() {
        let (_t, pool) = open_pool().await;
        let rule = SmartRule {
            combine: Combine::Any,
            conditions: vec![Condition { field: Field::Loved, op: Op::Eq, value: "1".into() }],
            limit: Some(50),
        };
        let id = create(&pool, "Faves", Some(&rule)).await.unwrap();
        assert_eq!(rule_of(&pool, id).await.unwrap().as_ref(), Some(&rule));

        let edited = SmartRule { limit: Some(10), ..rule.clone() };
        update_smart(&pool, id, "Faves, fewer", &edited).await.unwrap();
        assert_eq!(rule_of(&pool, id).await.unwrap(), Some(edited));
        let name: String = sqlx::query_scalar("SELECT name FROM playlists WHERE id = ?")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(name, "Faves, fewer");

        // A manual playlist has no rule, and that is not an error.
        let manual = create(&pool, "By hand", None).await.unwrap();
        assert!(rule_of(&pool, manual).await.unwrap().is_none());
    }

    #[test]
    fn every_field_and_op_is_offerable() {
        // The editor builds its dropdowns from these, so a field missing from
        // `all()` is a field nobody can ever author a condition on.
        assert_eq!(Field::all().len(), 10);
        assert_eq!(Op::all().len(), 7);
        // Every id round-trips, or an editor's dropdown saves a rule that
        // reloads as something else.
        for (f, id, _) in Field::all() {
            assert_eq!(Field::from_id(id).as_ref(), Some(f));
            assert_eq!(f.id(), *id);
        }
        for (o, id, _) in Op::all() {
            assert_eq!(Op::from_id(id).as_ref(), Some(o));
            assert_eq!(o.id(), *id);
        }
        assert!(Field::from_id("nonsense").is_none());
        for (f, id, label) in Field::all() {
            assert!(!id.is_empty() && !label.is_empty());
            assert!(matches!(f.kind(), "text" | "number" | "bool" | "days"));
        }
    }

    #[tokio::test]
    async fn manual_append_orders() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        let pl = create(&pool, "Mix", None).await.unwrap();
        append(&pool, pl, a).await.unwrap();
        append(&pool, pl, b).await.unwrap();
        assert_eq!(items(&pool, pl).await.unwrap(), vec![a, b]);
    }

    #[test]
    fn m3u_roundtrip() {
        let base = std::path::Path::new("/music/lib");
        let doc = "#EXTM3U\n#EXTINF:120,Song\n/abs/a.flac\nrel/b.mp3\n\n# comment\n";
        let paths = parse_m3u(doc, base);
        assert_eq!(paths, vec![
            std::path::PathBuf::from("/abs/a.flac"),
            std::path::PathBuf::from("/music/lib/rel/b.mp3"),
        ]);
        let out = write_m3u(&paths);
        assert!(out.starts_with("#EXTM3U\n"));
        assert!(out.contains("/abs/a.flac"));
    }

    #[test]
    fn pls_parses_fileN_in_order() {
        let base = std::path::Path::new("/music/lib");
        let doc = "[playlist]\nFile2=rel/b.mp3\nTitle2=B\nFile1=/abs/a.flac\nTitle1=A\nNumberOfEntries=2\n";
        let paths = parse_pls(doc, base);
        assert_eq!(paths, vec![
            std::path::PathBuf::from("/abs/a.flac"),
            std::path::PathBuf::from("/music/lib/rel/b.mp3"),
        ]);
        // http(s) stream entries are skipped (local-library import only).
        assert!(parse_pls("[playlist]\nFile1=https://stream.example/live\n", base).is_empty());
    }

    #[tokio::test]
    async fn smart_rule_filters() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        sqlx::query("UPDATE track_meta SET rating = 5, genre = 'Rock' WHERE item_id = ?").bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET rating = 2, genre = 'Jazz' WHERE item_id = ?").bind(b).execute(&pool).await.unwrap();
        let rule = SmartRule {
            combine: Combine::All,
            conditions: vec![
                Condition { field: Field::Rating, op: Op::Gte, value: "4".into() },
                Condition { field: Field::Genre, op: Op::Contains, value: "Roc".into() },
            ],
            limit: None,
        };
        let got = evaluate(&pool, &rule).await.unwrap();
        assert_eq!(got, vec![a]);
        // round-trips through JSON (smart playlist persistence)
        let json = serde_json::to_string(&rule).unwrap();
        let back: SmartRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rule);
    }

    #[tokio::test]
    async fn smart_rules_skip_audiobooks() {
        let (_t, pool) = open_pool().await;
        let song = add_track(&pool, "/m/song.flac").await;
        let chap = add_track(&pool, "/books/ch1.m4b").await;
        sqlx::query("UPDATE track_meta SET is_audiobook = 1 WHERE item_id = ?").bind(chap).execute(&pool).await.unwrap();
        // The match-everything "Recently Added" rule must still skip chapters.
        let rule = SmartRule { combine: Combine::All, conditions: vec![], limit: Some(100) };
        assert_eq!(evaluate(&pool, &rule).await.unwrap(), vec![song]);
    }

    #[tokio::test]
    async fn delete_and_find_by_name() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let pl = create(&pool, "Road Trip", None).await.unwrap();
        append(&pool, pl, a).await.unwrap();
        assert_eq!(find_by_name(&pool, "Road Trip").await.unwrap(), Some(pl));
        assert_eq!(find_by_name(&pool, "Nope").await.unwrap(), None);
        delete(&pool, pl).await.unwrap();
        assert_eq!(find_by_name(&pool, "Road Trip").await.unwrap(), None);
        assert!(items(&pool, pl).await.unwrap().is_empty());
    }
}
