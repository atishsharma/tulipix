//! The Arcade section's store and the launchers' files it reads: Steam's VDF,
//! Heroic's JSON, Lutris' database, `.desktop` entries, ROM folders. Nothing
//! here writes to a launcher; games are started through each one's own link,
//! so updates, DRM and cloud saves keep working. `api::arcade` maps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
-- What Arcade adds to a game: a favourite, hidden, a note.
CREATE TABLE IF NOT EXISTS marks (
    key    TEXT PRIMARY KEY,
    fav    INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    note   TEXT    NOT NULL DEFAULT ''
);

-- Play sessions Arcade timed itself: native games and ROMs.
CREATE TABLE IF NOT EXISTS sessions (
    key   TEXT    NOT NULL,
    start INTEGER NOT NULL,
    secs  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_key_idx ON sessions(key);

-- ProtonDB's tier per Steam app, asked once a week at most.
CREATE TABLE IF NOT EXISTS proton (
    appid   INTEGER PRIMARY KEY,
    tier    TEXT    NOT NULL,
    fetched INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS roms (
    id   INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One game, from whichever launcher has it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Game {
    /// "steam:570", "epic:Fortnite", "gog:1207658924", "lutris:12",
    /// "native:/usr/share/applications/supertux2.desktop", "rom:/…/zelda.sfc".
    pub key: String,
    pub title: String,
    /// steam | epic | gog | lutris | native | rom.
    pub source: String,
    /// "SNES", "Wine", "Linux"; "" when the source says it all.
    pub system: String,
    /// A picture file for the tile, or "".
    pub cover: String,
    pub minutes: i64,
    pub last_played: i64,
    pub size: i64,
    /// A link a launcher answers (steam://, heroic://, lutris:), or a path.
    pub launch: String,
    /// A command line to run and time, for games with no launcher.
    pub exec: String,
}

// ── Steam's VDF ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Map(Vec<(String, Vdf)>),
}

impl Vdf {
    /// A child by key, ignoring case (Steam writes "Steam" and "steam").
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Map(m) => m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Map(_) => None,
        }
    }

    pub fn path(&self, keys: &[&str]) -> Option<&Vdf> {
        keys.iter().try_fold(self, |v, k| v.get(k))
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Map(m) => m,
            Vdf::Str(_) => &[],
        }
    }
}

enum Tok {
    Text(String),
    Open,
    Close,
}

/// Valve's text KeyValues: quoted keys and values, braces, `//` comments.
pub fn parse_vdf(text: &str) -> Vdf {
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(o) => s.push(o),
                            None => {}
                        },
                        '"' => break,
                        o => s.push(o),
                    }
                }
                toks.push(Tok::Text(s));
            }
            '{' => toks.push(Tok::Open),
            '}' => toks.push(Tok::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    // key value | key { … } — until a close or the end.
    fn block(toks: &[Tok], i: &mut usize) -> Vec<(String, Vdf)> {
        let mut out = Vec::new();
        while let Some(t) = toks.get(*i) {
            *i += 1;
            let key = match t {
                Tok::Close => break,
                Tok::Open => continue,
                Tok::Text(k) => k.clone(),
            };
            match toks.get(*i) {
                Some(Tok::Text(v)) => {
                    *i += 1;
                    out.push((key, Vdf::Str(v.clone())));
                }
                Some(Tok::Open) => {
                    *i += 1;
                    out.push((key, Vdf::Map(block(toks, i))));
                }
                _ => break,
            }
        }
        out
    }
    let mut i = 0;
    Vdf::Map(block(&toks, &mut i))
}

/// Where Steam is, on this computer.
pub fn steam_roots(home: &Path) -> Vec<PathBuf> {
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        PathBuf::from("C:\\Program Files (x86)\\Steam"),
        home.join("Library/Application Support/Steam"),
    ]
    .into_iter()
    .filter(|p| p.join("steamapps").is_dir())
    .collect()
}

/// Not games: Proton, the runtimes, the redistributables.
fn steam_tool(appid: &str, name: &str) -> bool {
    matches!(appid, "228980" | "1070560" | "1391110" | "1628350" | "1493710" | "2180100")
        || name.starts_with("Proton")
        || name.starts_with("Steam Linux Runtime")
        || name.starts_with("Steamworks")
}

/// The library folders `libraryfolders.vdf` lists, the root among them.
pub fn steam_libraries(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    if let Ok(t) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        let v = parse_vdf(&t);
        if let Some(lf) = v.get("libraryfolders") {
            for (_, lib) in lf.entries() {
                if let Some(p) = lib.str("path").map(PathBuf::from)
                    && !out.contains(&p)
                {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// A Steam app from its manifest: (appid, name, size on disk, last played).
pub fn parse_acf(text: &str) -> Option<(String, String, i64, i64)> {
    let v = parse_vdf(text);
    let a = v.get("AppState")?;
    let id = a.str("appid")?.to_string();
    let name = a.str("name")?.to_string();
    let n = |k: &str| a.str(k).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    Some((id, name, n("SizeOnDisk"), n("LastPlayed")))
}

/// Minutes and last played per app, from every Steam user on this computer.
pub fn steam_playtime(root: &Path) -> HashMap<String, (i64, i64)> {
    let mut out: HashMap<String, (i64, i64)> = HashMap::new();
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else { return out };
    for u in users.filter_map(|e| e.ok()) {
        let Ok(t) = std::fs::read_to_string(u.path().join("config/localconfig.vdf")) else { continue };
        let v = parse_vdf(&t);
        let Some(apps) = v.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]) else { continue };
        for (id, app) in apps.entries() {
            let n = |k: &str| app.str(k).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
            let e = out.entry(id.clone()).or_default();
            e.0 += n("Playtime");
            e.1 = e.1.max(n("LastPlayed"));
        }
    }
    out
}

/// The tall cover Steam keeps for an app, in its old layout or its new one.
pub fn steam_cover(root: &Path, appid: &str) -> String {
    let cache = root.join("appcache/librarycache");
    let flat = cache.join(format!("{appid}_library_600x900.jpg"));
    if flat.exists() {
        return flat.to_string_lossy().to_string();
    }
    let dir = cache.join(appid);
    let direct = dir.join("library_600x900.jpg");
    if direct.exists() {
        return direct.to_string_lossy().to_string();
    }
    // Newer clients file some art under a hash folder.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path().join("library_600x900.jpg");
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }
    }
    String::new()
}

pub fn steam_games(home: &Path) -> Vec<Game> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in steam_roots(home) {
        let played = steam_playtime(&root);
        for lib in steam_libraries(&root) {
            let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().to_string();
                if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
                let Some((id, title, size, last)) = parse_acf(&t) else { continue };
                if steam_tool(&id, &title) || !seen.insert(id.clone()) {
                    continue;
                }
                let (minutes, last2) = played.get(&id).copied().unwrap_or((0, 0));
                out.push(Game {
                    key: format!("steam:{id}"),
                    title,
                    source: "steam".into(),
                    cover: steam_cover(&root, &id),
                    minutes,
                    last_played: last.max(last2),
                    size,
                    launch: format!("steam://rungameid/{id}"),
                    ..Game::default()
                });
            }
        }
    }
    out
}

// ── Heroic ──────────────────────────────────────────────────────────────────

pub fn heroic_root(home: &Path) -> Option<PathBuf> {
    [home.join(".config/heroic"), home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic")]
        .into_iter()
        .find(|p| p.is_dir())
}

/// Heroic's play record: minutes and last played per app name.
pub fn heroic_playtime(text: &str) -> HashMap<String, (i64, i64)> {
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(text) else { return HashMap::new() };
    m.into_iter()
        .map(|(k, v)| {
            let minutes = v["totalPlayed"].as_f64().unwrap_or(0.0) as i64;
            let last = v["lastPlayed"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp())
                .unwrap_or(0);
            (k, (minutes, last))
        })
        .collect()
}

/// Epic games Heroic installed, through legendary: (app name, title, size).
pub fn parse_legendary(text: &str) -> Vec<(String, String, i64)> {
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    m.into_iter()
        .filter_map(|(k, v)| {
            let title = v["title"].as_str().unwrap_or(&k).to_string();
            (!v["is_dlc"].as_bool().unwrap_or(false)).then(|| (k, title, v["install_size"].as_i64().unwrap_or(0)))
        })
        .collect()
}

/// GOG games Heroic installed: (app name, title from the install folder).
pub fn parse_gog_installed(text: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    v["installed"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let app = g["appName"].as_str()?.to_string();
                    let folder = g["install_path"].as_str().map(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).unwrap_or_default();
                    Some((app, folder))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn heroic_games(home: &Path) -> Vec<Game> {
    let Some(root) = heroic_root(home) else { return Vec::new() };
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let played = heroic_playtime(&read("store/timestamp.json"));
    // GOG titles, when Heroic cached the library.
    let titles: HashMap<String, String> = serde_json::from_str::<serde_json::Value>(&read("store_cache/gog_library.json"))
        .ok()
        .and_then(|v| v["games"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|g| Some((g["app_name"].as_str()?.to_string(), g["title"].as_str()?.to_string())))
        .collect();
    let mut out = Vec::new();
    for (app, title, size) in parse_legendary(&read("legendaryConfig/legendary/installed.json")) {
        let (minutes, last) = played.get(&app).copied().unwrap_or((0, 0));
        out.push(Game {
            key: format!("epic:{app}"),
            title,
            source: "epic".into(),
            minutes,
            last_played: last,
            size,
            launch: format!("heroic://launch/legendary/{app}"),
            ..Game::default()
        });
    }
    for (app, folder) in parse_gog_installed(&read("gog_store/installed.json")) {
        let (minutes, last) = played.get(&app).copied().unwrap_or((0, 0));
        out.push(Game {
            key: format!("gog:{app}"),
            title: titles.get(&app).cloned().unwrap_or(folder),
            source: "gog".into(),
            minutes,
            last_played: last,
            launch: format!("heroic://launch/gog/{app}"),
            ..Game::default()
        });
    }
    out
}

// ── Lutris ──────────────────────────────────────────────────────────────────

pub fn lutris_db(home: &Path) -> Option<PathBuf> {
    [home.join(".local/share/lutris/pga.db"), home.join(".var/app/net.lutris.Lutris/data/lutris/pga.db")]
        .into_iter()
        .find(|p| p.exists())
}

/// Lutris' games, read without touching its database.
pub async fn lutris_games(home: &Path) -> Vec<Game> {
    let Some(db) = lutris_db(home) else { return Vec::new() };
    let data = db.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&db).read_only(true);
    let Ok(pool) = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(opts).await else { return Vec::new() };
    let rows: Vec<(i64, String, String, Option<String>, Option<f64>, Option<i64>)> = sqlx::query_as(
        "SELECT id, COALESCE(name, ''), COALESCE(slug, ''), runner, playtime, lastplayed FROM games WHERE installed = 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    pool.close().await;
    rows.into_iter()
        .map(|(id, name, slug, runner, hours, last)| {
            let cover = [data.join(format!("coverart/{slug}.jpg")), home.join(format!(".cache/lutris/coverart/{slug}.jpg"))]
                .into_iter()
                .find(|p| p.exists())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            Game {
                key: format!("lutris:{id}"),
                title: name,
                source: "lutris".into(),
                system: match runner.as_deref() {
                    Some("wine") => "Wine".into(),
                    Some("linux") => "Linux".into(),
                    Some(r) => r.to_string(),
                    None => String::new(),
                },
                cover,
                minutes: (hours.unwrap_or(0.0) * 60.0).round() as i64,
                last_played: last.unwrap_or(0),
                launch: format!("lutris:rungameid/{id}"),
                ..Game::default()
            }
        })
        .collect()
}

// ── .desktop games ──────────────────────────────────────────────────────────

/// A desktop entry's Name, Exec and Icon when it is a game and not one of the
/// launchers' own shortcuts.
pub fn parse_desktop(text: &str) -> Option<(String, String, String)> {
    let mut in_entry = false;
    let (mut name, mut exec, mut icon, mut cats) = (String::new(), String::new(), String::new(), String::new());
    let mut hidden = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        match k.trim() {
            "Name" => name = v.trim().to_string(),
            "Exec" => exec = v.trim().to_string(),
            "Icon" => icon = v.trim().to_string(),
            "Categories" => cats = v.to_string(),
            "NoDisplay" | "Hidden" if v.trim() == "true" => hidden = true,
            _ => {}
        }
    }
    let launcher = ["steam://", "heroic://", "lutris:", "steam ", "heroic ", "lutris "].iter().any(|p| exec.contains(p));
    let game = cats.split(';').any(|c| c == "Game");
    (game && !hidden && !launcher && !name.is_empty() && !exec.is_empty()).then(|| (name, exec, icon))
}

/// Exec without its field codes (%U, %f…), ready for `sh -c`.
pub fn exec_line(exec: &str) -> String {
    exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect::<Vec<_>>().join(" ")
}

pub fn native_games(home: &Path) -> Vec<Game> {
    let mut out = Vec::new();
    for dir in [home.join(".local/share/applications"), PathBuf::from("/usr/share/applications"), PathBuf::from("/var/lib/flatpak/exports/share/applications")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(&p) else { continue };
            let Some((name, exec, _icon)) = parse_desktop(&t) else { continue };
            out.push(Game {
                key: format!("native:{}", p.to_string_lossy()),
                title: name,
                source: "native".into(),
                system: "Linux".into(),
                exec: exec_line(&exec),
                ..Game::default()
            });
        }
    }
    out
}

// ── ROMs ────────────────────────────────────────────────────────────────────

/// The system a ROM is for, from its extension.
pub fn rom_system(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_lowercase();
    Some(match ext.as_str() {
        "nes" => "NES",
        "sfc" | "smc" => "SNES",
        "gb" => "Game Boy",
        "gbc" => "Game Boy Color",
        "gba" => "Game Boy Advance",
        "n64" | "z64" | "v64" => "Nintendo 64",
        "nds" => "Nintendo DS",
        "md" | "gen" | "smd" => "Mega Drive",
        "sms" => "Master System",
        "gg" => "Game Gear",
        "pce" => "PC Engine",
        "a26" => "Atari 2600",
        "chd" | "cue" => "Disc",
        "rvz" | "gcm" => "GameCube",
        "wbfs" => "Wii",
        "cso" => "PSP",
        _ => return None,
    })
}

/// A title from a ROM's file name: "Chrono Trigger (USA) [!].sfc" is
/// "Chrono Trigger".
pub fn rom_title(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let cut = stem.find(['(', '[']).unwrap_or(stem.len());
    stem[..cut].replace('_', " ").trim().to_string()
}

pub fn rom_games(folders: &[String]) -> Vec<Game> {
    let mut out = Vec::new();
    for f in folders {
        let mut stack = vec![PathBuf::from(f)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                let Some(system) = rom_system(&name) else { continue };
                let size = e.metadata().map(|m| m.len() as i64).unwrap_or(0);
                out.push(Game {
                    key: format!("rom:{}", p.to_string_lossy()),
                    title: rom_title(&name),
                    source: "rom".into(),
                    system: system.into(),
                    size,
                    launch: p.to_string_lossy().to_string(),
                    ..Game::default()
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_manifests_and_playtime() {
        let acf = r#""AppState"
{
	"appid"		"570"
	"name"		"Dota \"2\""
	"SizeOnDisk"		"38000000000"
	"LastPlayed"		"1700000000"
	"UserConfig" { "language" "english" }
}"#;
        assert_eq!(parse_acf(acf), Some(("570".into(), "Dota \"2\"".into(), 38_000_000_000, 1_700_000_000)));
        let lc = r#"// a comment
"UserLocalConfigStore" { "Software" { "Valve" { "steam" { "apps" {
    "570" { "LastPlayed" "1700000001" "Playtime" "125" }
    "440" { "Playtime" "7" }
} } } } }"#;
        let v = parse_vdf(lc);
        let apps = v.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]).unwrap();
        assert_eq!(apps.entries().len(), 2);
        assert_eq!(apps.get("570").and_then(|a| a.str("Playtime")), Some("125"));
        let lf = parse_vdf(r#""libraryfolders" { "0" { "path" "/home/u/.local/share/Steam" "apps" { "228980" "1" } } "1" { "path" "/mnt/games" } }"#);
        let paths: Vec<&str> = lf.get("libraryfolders").unwrap().entries().iter().filter_map(|(_, l)| l.str("path")).collect();
        assert_eq!(paths, vec!["/home/u/.local/share/Steam", "/mnt/games"]);
        assert!(steam_tool("1628350", "Steam Linux Runtime 3.0") && !steam_tool("570", "Dota 2"));
    }

    #[test]
    fn heroic_files() {
        let t = heroic_playtime(r#"{"Fortnite":{"firstPlayed":"2023-01-01T10:00:00.000Z","lastPlayed":"2023-11-14T22:13:20.000Z","totalPlayed":95}}"#);
        assert_eq!(t["Fortnite"], (95, 1_700_000_000));
        let l = parse_legendary(r#"{"Fortnite":{"title":"Fortnite","install_size":1000,"is_dlc":false},"X":{"title":"X DLC","is_dlc":true}}"#);
        assert_eq!(l, vec![("Fortnite".to_string(), "Fortnite".to_string(), 1000)]);
        let g = parse_gog_installed(r#"{"installed":[{"appName":"1207658924","install_path":"/games/Pentiment"}]}"#);
        assert_eq!(g, vec![("1207658924".to_string(), "Pentiment".to_string())]);
    }

    #[test]
    fn desktop_entries_and_roms() {
        let d = "[Desktop Entry]\nName=SuperTux\nExec=supertux2 %U\nIcon=supertux\nCategories=Game;ArcadeGame;\n\n[Desktop Action New]\nName=Other\n";
        assert_eq!(parse_desktop(d), Some(("SuperTux".into(), "supertux2 %U".into(), "supertux".into())));
        assert_eq!(exec_line("supertux2 %U --fullscreen"), "supertux2 --fullscreen");
        assert_eq!(parse_desktop("[Desktop Entry]\nName=Hades\nExec=steam steam://rungameid/1145360\nCategories=Game;\n"), None);
        assert_eq!(parse_desktop("[Desktop Entry]\nName=Files\nExec=nautilus\nCategories=Utility;\n"), None);
        assert_eq!(rom_system("Chrono Trigger (USA).sfc"), Some("SNES"));
        assert_eq!(rom_system("notes.txt"), None);
        assert_eq!(rom_title("Chrono Trigger (USA) [!].sfc"), "Chrono Trigger");
        assert_eq!(rom_title("super_mario_world.smc"), "super mario world");
    }
}
