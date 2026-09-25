// The Arcade section: every game already on this computer on one shelf.
//
// Three tabs — Library (with a game open), Stats and Sources. Games come from
// Steam, Heroic (Epic and GOG), Lutris, desktop entries in the Games category
// and ROM folders you add (`crate::arcade` reads them). A game starts through
// its own launcher's link; play time is the launcher's own, and Arcade times
// what it starts itself. ProtonDB tiers are fetched only when switched on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use chrono::{Local, TimeZone};
use sqlx::SqlitePool;

use crate::arcade::{self as g, Game};
use crate::db::arcade_pool;

// ------------------------------------------------------------------- state ---

pub struct ArcadeState {
    /// library | stats | sources.
    pub tab: String,
    pub notice: String,
    /// all | recent | fav | unplayed | steam | epic | gog | lutris | native | rom | hidden.
    pub filter: String,
    pub query: String,
    pub games: Vec<GameTile>,
    pub counts: Vec<FilterCount>,
    pub open: Option<GameView>,
    pub sources: Vec<GameSource>,
    pub roms: Vec<RomFolder>,
    pub stats: ArcadeStats,
    pub proton: bool,
    /// ludusavi is on PATH: saves can be backed up.
    pub ludusavi: bool,
    /// The game to go back to: the last played.
    pub resume: Option<GameTile>,
}

pub struct GameTile {
    pub key: String,
    pub title: String,
    pub source: String,
    pub system: String,
    pub cover: String,
    /// "42 h", "35 min", "".
    pub played: String,
    pub minutes: i64,
    /// "Yesterday", "Mar 2024", "".
    pub last: String,
    pub fav: bool,
}

pub struct FilterCount {
    pub id: String,
    pub label: String,
    pub n: i64,
}

pub struct GameView {
    pub key: String,
    pub title: String,
    /// "Steam", "Epic, through Heroic".
    pub source_label: String,
    pub system: String,
    pub cover: String,
    pub played: String,
    pub last: String,
    pub size: String,
    pub note: String,
    pub fav: bool,
    pub hidden: bool,
    /// platinum | gold | silver | bronze | borked | native | "" (unknown or off).
    pub tier: String,
    /// Sessions Arcade timed: "Tue 23 Sep · 1 h 20 min".
    pub sessions: Vec<String>,
}

pub struct GameSource {
    pub id: String,
    pub label: String,
    pub found: bool,
    pub games: i64,
    pub how: String,
}

pub struct RomFolder {
    pub id: i64,
    pub path: String,
    pub games: i64,
}

#[derive(Default)]
pub struct ArcadeStats {
    pub total: String,
    pub games: i64,
    pub played: i64,
    pub never: i64,
    pub top: Vec<GameTile>,
    pub by_source: Vec<FilterCount>,
}

// ---------------------------------------------------------------- commands ---

pub enum ArcadeCmd {
    Refresh,
    /// Read the launchers again now rather than in a minute.
    Rescan,
    SetTab { tab: String },
    SetFilter { filter: String },
    Search { text: String },
    Open { key: String },
    Close,
    Play { key: String },
    Fav { key: String },
    Hide { key: String },
    SetNote { key: String, note: String },
    AddRoms { path: String },
    RemoveRoms { id: i64 },
    /// Ask ProtonDB about Steam games (sends their app ids).
    SetProton { on: bool },
    /// Back the game's saves up with ludusavi.
    BackupSaves { key: String },
}

// ----------------------------------------------------------------- session ---

/// `frb(ignore)`: private state, not part of the contract.
#[flutter_rust_bridge::frb(ignore)]
struct Session {
    tab: String,
    filter: String,
    query: String,
    open: String,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "library".into(),
            filter: "all".into(),
            query: String::new(),
            open: String::new(),
            notice: String::new(),
        })
    })
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn say(notice: impl Into<String>) {
    lock().notice = notice.into();
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

// ------------------------------------------------------------------- games ---

fn cache() -> &'static Mutex<Option<(Instant, Arc<Vec<Game>>)>> {
    static C: OnceLock<Mutex<Option<(Instant, Arc<Vec<Game>>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Every source read, at most once a minute.
async fn games(pool: &SqlitePool, fresh: bool) -> Result<Arc<Vec<Game>>> {
    if !fresh
        && let Some((at, list)) = cache().lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(list.clone());
    }
    let h = home();
    let folders: Vec<String> = sqlx::query_scalar("SELECT path FROM roms ORDER BY id").fetch_all(pool).await?;
    let lutris = g::lutris_games(&h).await;
    let h2 = h.clone();
    let mut all = tokio::task::spawn_blocking(move || {
        let mut v = g::steam_games(&h2);
        v.extend(g::heroic_games(&h2));
        v.extend(g::native_games(&h2));
        v.extend(g::rom_games(&folders));
        v
    })
    .await?;
    all.extend(lutris);
    // Arcade's own timing, for what it started itself.
    let timed: HashMap<String, (i64, i64)> = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT key, SUM(secs), MAX(start + secs) FROM sessions GROUP BY key",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(k, s, l)| (k, (s, l)))
    .collect();
    for x in all.iter_mut() {
        if let Some((secs, last)) = timed.get(&x.key) {
            x.minutes += secs / 60;
            x.last_played = x.last_played.max(*last);
        }
    }
    all.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    let list = Arc::new(all);
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

async fn find(pool: &SqlitePool, key: &str) -> Result<Game> {
    games(pool, false).await?.iter().find(|x| x.key == key).cloned().ok_or_else(|| anyhow!("that game is not here any more"))
}

fn played(minutes: i64) -> String {
    match minutes {
        0 => String::new(),
        m if m < 60 => format!("{m} min"),
        m if m < 600 => format!("{:.1} h", m as f64 / 60.0).replace(".0 h", " h"),
        m => format!("{} h", m / 60),
    }
}

fn last_label(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let Some(t) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    match (Local::now().date_naive() - t.date_naive()).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        n if n < 7 => t.format("%A").to_string(),
        n if n < 300 => t.format("%-d %b").to_string(),
        _ => t.format("%b %Y").to_string(),
    }
}

fn source_label(s: &str) -> &'static str {
    match s {
        "steam" => "Steam",
        "epic" => "Epic, through Heroic",
        "gog" => "GOG, through Heroic",
        "lutris" => "Lutris",
        "native" => "Installed on this computer",
        "rom" => "A ROM",
        _ => "",
    }
}

// ---------------------------------------------------------------- exported ---

pub async fn arcade_dispatch(cmd: ArcadeCmd) -> Result<ArcadeState> {
    let pool = arcade_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: ArcadeCmd) -> Result<()> {
    match cmd {
        ArcadeCmd::Refresh => {}
        ArcadeCmd::Rescan => {
            games(pool, true).await?;
        }
        ArcadeCmd::SetTab { tab } => {
            let mut s = lock();
            s.tab = tab;
            s.open.clear();
        }
        ArcadeCmd::SetFilter { filter } => lock().filter = filter,
        ArcadeCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            s.tab = "library".into();
        }
        ArcadeCmd::Open { key } => {
            let mut s = lock();
            s.open = key;
            s.tab = "library".into();
        }
        ArcadeCmd::Close => lock().open.clear(),
        ArcadeCmd::Play { key } => {
            let game = find(pool, &key).await?;
            if !game.exec.is_empty() {
                let child = tokio::process::Command::new("sh")
                    .args(["-c", &game.exec])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| anyhow!("could not start {}: {e}", game.title))?;
                // Timed while it runs; the session counts once it ends.
                let start = g::now();
                tokio::spawn(async move {
                    let mut child = child;
                    child.wait().await.ok();
                    let secs = g::now() - start;
                    if secs >= 30 {
                        sqlx::query("INSERT INTO sessions (key, start, secs) VALUES (?, ?, ?)")
                            .bind(&key)
                            .bind(start)
                            .bind(secs)
                            .execute(pool)
                            .await
                            .ok();
                        *cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                });
            } else {
                crate::api::transfer::open_url(&game.launch);
            }
            say(format!("Starting {}", game.title));
        }
        ArcadeCmd::Fav { key } => {
            sqlx::query("INSERT INTO marks (key, fav) VALUES (?, 1) ON CONFLICT(key) DO UPDATE SET fav = 1 - fav").bind(key).execute(pool).await?;
        }
        ArcadeCmd::Hide { key } => {
            sqlx::query("INSERT INTO marks (key, hidden) VALUES (?, 1) ON CONFLICT(key) DO UPDATE SET hidden = 1 - hidden")
                .bind(key)
                .execute(pool)
                .await?;
        }
        ArcadeCmd::SetNote { key, note } => {
            sqlx::query("INSERT INTO marks (key, note) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET note = excluded.note")
                .bind(key)
                .bind(note.trim())
                .execute(pool)
                .await?;
        }
        ArcadeCmd::AddRoms { path } => {
            if !std::path::Path::new(path.trim()).is_dir() {
                bail!("{} is not a folder", path.trim());
            }
            sqlx::query("INSERT OR IGNORE INTO roms (path) VALUES (?)").bind(path.trim()).execute(pool).await?;
            games(pool, true).await?;
        }
        ArcadeCmd::RemoveRoms { id } => {
            sqlx::query("DELETE FROM roms WHERE id = ?").bind(id).execute(pool).await?;
            games(pool, true).await?;
        }
        ArcadeCmd::SetProton { on } => crate::api::shell::put("arcade.protondb", if on { "true" } else { "" }),
        ArcadeCmd::BackupSaves { key } => {
            let game = find(pool, &key).await?;
            if !tulipix_common::on_path("ludusavi") {
                bail!("ludusavi is not installed — it knows where 19,000 games keep their saves");
            }
            let out = tokio::process::Command::new("ludusavi").args(["backup", "--force"]).arg(&game.title).output().await?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                bail!("ludusavi could not: {}", err.lines().last().unwrap_or("it did not say why"));
            }
            say(format!("Saves for {} backed up by ludusavi", game.title));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ proton ---

/// A Steam game's ProtonDB tier, asked once a week at most, and only when
/// switched on.
async fn tier(pool: &SqlitePool, appid: i64) -> String {
    let week_ago = g::now() - 7 * 86_400;
    if let Ok(Some((t, at))) =
        sqlx::query_as::<_, (String, i64)>("SELECT tier, fetched FROM proton WHERE appid = ?").bind(appid).fetch_optional(pool).await
        && at > week_ago
    {
        return t;
    }
    let url = format!("https://www.protondb.com/api/v1/reports/summaries/{appid}.json");
    let got = async {
        let v: serde_json::Value = crate::feeds::client().get(url).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
        v["tier"].as_str().map(str::to_string)
    }
    .await
    .unwrap_or_default();
    sqlx::query("INSERT OR REPLACE INTO proton (appid, tier, fetched) VALUES (?, ?, ?)")
        .bind(appid)
        .bind(&got)
        .bind(g::now())
        .execute(pool)
        .await
        .ok();
    got
}

// ---------------------------------------------------------------- snapshot ---

fn tile(x: &Game, fav: bool) -> GameTile {
    GameTile {
        key: x.key.clone(),
        title: x.title.clone(),
        source: x.source.clone(),
        system: x.system.clone(),
        cover: x.cover.clone(),
        played: played(x.minutes),
        minutes: x.minutes,
        last: last_label(x.last_played),
        fav,
    }
}

async fn snapshot(pool: &SqlitePool) -> Result<ArcadeState> {
    let (tab, filter, query, open, notice) = {
        let mut s = lock();
        (s.tab.clone(), s.filter.clone(), s.query.clone(), s.open.clone(), std::mem::take(&mut s.notice))
    };
    let all = games(pool, false).await?;
    let marks: HashMap<String, (bool, bool, String)> = sqlx::query_as::<_, (String, i64, i64, String)>("SELECT key, fav, hidden, note FROM marks")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(k, f, h, n)| (k, (f != 0, h != 0, n)))
        .collect();
    let fav = |k: &str| marks.get(k).is_some_and(|m| m.0);
    let hidden = |k: &str| marks.get(k).is_some_and(|m| m.1);

    let visible: Vec<&Game> = all.iter().filter(|x| !hidden(&x.key)).collect();
    let q = query.to_lowercase();
    let mut shown: Vec<&Game> = match filter.as_str() {
        "hidden" => all.iter().filter(|x| hidden(&x.key)).collect(),
        "fav" => visible.iter().copied().filter(|x| fav(&x.key)).collect(),
        "unplayed" => visible.iter().copied().filter(|x| x.minutes == 0).collect(),
        "recent" => {
            let mut v: Vec<&Game> = visible.iter().copied().filter(|x| x.last_played > 0).collect();
            v.sort_by(|a, b| b.last_played.cmp(&a.last_played));
            v
        }
        "all" => visible.clone(),
        src => visible.iter().copied().filter(|x| x.source == src).collect(),
    };
    if !q.is_empty() {
        shown.retain(|x| x.title.to_lowercase().contains(&q));
    }
    let games: Vec<GameTile> = shown.iter().map(|x| tile(x, fav(&x.key))).collect();

    let n = |f: &dyn Fn(&&Game) -> bool| visible.iter().filter(|x| f(x)).count() as i64;
    let mut counts = vec![
        FilterCount { id: "all".into(), label: "All".into(), n: visible.len() as i64 },
        FilterCount { id: "recent".into(), label: "Recently played".into(), n: n(&|x| x.last_played > 0) },
        FilterCount { id: "fav".into(), label: "Favourites".into(), n: n(&|x| fav(&x.key)) },
        FilterCount { id: "unplayed".into(), label: "Not played yet".into(), n: n(&|x| x.minutes == 0) },
    ];
    for (id, label) in [("steam", "Steam"), ("epic", "Epic"), ("gog", "GOG"), ("lutris", "Lutris"), ("native", "Linux"), ("rom", "ROMs")] {
        let k = n(&|x| x.source == id);
        if k > 0 {
            counts.push(FilterCount { id: id.into(), label: label.into(), n: k });
        }
    }
    let hid = all.iter().filter(|x| hidden(&x.key)).count() as i64;
    if hid > 0 {
        counts.push(FilterCount { id: "hidden".into(), label: "Hidden".into(), n: hid });
    }

    let proton = crate::api::shell::load().flag("arcade.protondb", false);
    let open = if open.is_empty() {
        None
    } else if let Some(x) = all.iter().find(|x| x.key == open) {
        let tier = match x.key.strip_prefix("steam:").and_then(|id| id.parse::<i64>().ok()) {
            Some(id) if proton => tier(pool, id).await,
            _ if x.source == "native" => "native".into(),
            _ => String::new(),
        };
        let sessions: Vec<String> = sqlx::query_as::<_, (i64, i64)>("SELECT start, secs FROM sessions WHERE key = ? ORDER BY start DESC LIMIT 12")
            .bind(&x.key)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(start, secs)| format!("{} · {}", last_label(start), or(played(secs / 60), "under a minute")))
            .collect();
        let m = marks.get(&x.key);
        Some(GameView {
            key: x.key.clone(),
            title: x.title.clone(),
            source_label: source_label(&x.source).into(),
            system: x.system.clone(),
            cover: x.cover.clone(),
            played: played(x.minutes),
            last: last_label(x.last_played),
            size: if x.size > 0 { crate::archive::size_label(x.size) } else { String::new() },
            note: m.map(|m| m.2.clone()).unwrap_or_default(),
            fav: m.is_some_and(|m| m.0),
            hidden: m.is_some_and(|m| m.1),
            tier,
            sessions,
        })
    } else {
        lock().open.clear();
        None
    };

    let h = home();
    let steam = g::steam_roots(&h);
    let heroic = g::heroic_root(&h);
    let lutris = g::lutris_db(&h);
    let per = |s: &str| all.iter().filter(|x| x.source == s).count() as i64;
    let sources = vec![
        GameSource {
            id: "steam".into(),
            label: "Steam".into(),
            found: !steam.is_empty(),
            games: per("steam"),
            how: steam.first().map(|p| format!("{} · installed games, play time from Steam", p.display())).unwrap_or_else(|| "Not found".into()),
        },
        GameSource {
            id: "heroic".into(),
            label: "Heroic — Epic and GOG".into(),
            found: heroic.is_some(),
            games: per("epic") + per("gog"),
            how: heroic.map(|p| format!("{} · started through Heroic", p.display())).unwrap_or_else(|| "Not found".into()),
        },
        GameSource {
            id: "lutris".into(),
            label: "Lutris".into(),
            found: lutris.is_some(),
            games: per("lutris"),
            how: lutris.map(|p| format!("{} · read, never written", p.display())).unwrap_or_else(|| "Not found".into()),
        },
        GameSource {
            id: "native".into(),
            label: "Games installed on this computer".into(),
            found: per("native") > 0,
            games: per("native"),
            how: "Desktop entries in the Games category · timed by Arcade".into(),
        },
    ];
    let rom_games: HashMap<String, i64> = {
        let mut m = HashMap::new();
        for x in all.iter().filter(|x| x.source == "rom") {
            *m.entry(x.launch.clone()).or_default() += 1;
        }
        m
    };
    let roms: Vec<RomFolder> = sqlx::query_as::<_, (i64, String)>("SELECT id, path FROM roms ORDER BY id")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, path)| {
            let games = rom_games.iter().filter(|(k, _)| k.starts_with(&path)).map(|(_, n)| n).sum();
            RomFolder { id, path, games }
        })
        .collect();

    let total_min: i64 = visible.iter().map(|x| x.minutes).sum();
    let mut top: Vec<&Game> = visible.iter().copied().filter(|x| x.minutes > 0).collect();
    top.sort_by(|a, b| b.minutes.cmp(&a.minutes));
    let mut by_source: Vec<FilterCount> = ["steam", "epic", "gog", "lutris", "native", "rom"]
        .into_iter()
        .map(|s| FilterCount {
            id: s.into(),
            label: source_label(s).into(),
            n: visible.iter().filter(|x| x.source == s).map(|x| x.minutes).sum(),
        })
        .filter(|f| f.n > 0)
        .collect();
    by_source.sort_by(|a, b| b.n.cmp(&a.n));
    let stats = ArcadeStats {
        total: or(played(total_min), "0 h"),
        games: visible.len() as i64,
        played: visible.iter().filter(|x| x.minutes > 0).count() as i64,
        never: visible.iter().filter(|x| x.minutes == 0).count() as i64,
        top: top.iter().take(8).map(|x| tile(x, fav(&x.key))).collect(),
        by_source,
    };
    let resume = visible.iter().filter(|x| x.last_played > 0).max_by_key(|x| x.last_played).map(|x| tile(x, fav(&x.key)));

    Ok(ArcadeState {
        tab,
        notice,
        filter,
        query,
        games,
        counts,
        open,
        sources,
        roms,
        stats,
        proton,
        ludusavi: tulipix_common::on_path("ludusavi"),
        resume,
    })
}

/// `s`, or `other` when `s` is empty.
fn or(s: String, other: &str) -> String {
    if s.is_empty() { other.to_string() } else { s }
}
