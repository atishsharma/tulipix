//! Settings: nine tabs over one `Settings` struct.
//!
//! Six of them are data-driven -- a list of rows, each a toggle, a text box, a
//! read-only status line or an action button, keyed by a stable setting id. New
//! settings need no schema change and no new command: they are a row in the
//! list below and a key in `flags` or `advanced`.
//!
//! The row *definitions* are the port. They live in the Slint build's
//! `main.rs`, which is a binary this cannot link, so they are transcribed here
//! -- same keys, same defaults, same copy. A key that drifted would silently
//! split one setting into two.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::api::shell::{load, put, save};

/// One row of a settings panel.
///
/// `kind` is what to draw: "header" | "toggle" | "text" | "status" | "action"
/// | "choice". `state` tints a status line: "ok" | "warn" | "error" | "busy" |
/// "muted".
pub struct SettingItem {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub desc: String,
    /// The text value, the action's button label, or the status line's reading.
    pub value: String,
    pub on: bool,
    pub state: String,
    /// Choice rows only: what `value` may be set to.
    pub options: Vec<String>,
    /// "status-action" rows only: the trailing button's label. A status row
    /// and its Download button are one row, not two, because they describe
    /// one thing and drifted apart the moment they were separate.
    pub btn: String,
    /// 0..1 while a download is running, so the row can draw a determinate
    /// bar. Negative means "no progress to show".
    pub frac: f64,
}

/// One watched folder.
pub struct LibraryRow {
    pub path: String,
    pub exists: bool,
    /// Which sections found something under it.
    pub sections: Vec<String>,
    /// Where its music goes: "mymusic" | "podcasts" | "audiobooks" | "radio"
    /// | "youtube". The shared `music_folder_sections.json`, which the Slint
    /// build writes too.
    pub music_section: String,
    /// Items indexed under it in photos, videos and music. -1 until counted.
    pub items: i64,
    /// When it was last read, Unix seconds. 0: not since this was kept.
    pub last_scan: i64,
    /// Its own auto-rescan cadence, `lib.cadence.<path>`: "manual" | "hourly"
    /// | "daily" | "weekly", or empty for the default.
    pub cadence: String,
}

/// A service key's row. The key itself stays in the system keychain and never
/// crosses the bridge -- only whether one is saved.
pub struct ServiceKeyRow {
    pub service: String,
    pub label: String,
    pub saved: bool,
}

/// The Home layout's card list — one switch per card.
pub struct HomeCardRow {
    pub key: String,
    pub label: String,
    pub on: bool,
}

/// One backup under `<data>/backups`. `id` is its folder name, the Unix time it
/// was made, and the only thing a Restore is allowed to name.
pub struct BackupRow {
    pub id: String,
    pub secs: i64,
    pub files: u32,
    pub bytes: u64,
}

pub struct SettingsState {
    /// "profile" | "libraries" | "playback" | "services" | "ai" | "security"
    /// | "data" | "advanced" | "status".
    pub tab: String,
    // You & Home.
    pub display_name: String,
    pub avatar_emoji: String,
    /// The cropped profile photo, or empty -- the emoji stands in.
    pub avatar_path: String,
    /// The cropped header cover, or empty -- the gradient stands in.
    pub cover_path: String,
    pub logo_choice: i32,
    pub theme: String,
    pub reduce_motion: bool,
    /// "classic" | "welcome" | "cinema" | "stream".
    pub home_layout: String,
    pub home_cards: Vec<HomeCardRow>,
    pub home_music_left: bool,
    pub app_version: String,
    // Libraries.
    pub libraries: Vec<LibraryRow>,
    pub default_cadence: String,
    /// `scan.exclude`: the patterns every scan passes over, as typed.
    pub exclusions: String,
    // The data-driven panels.
    pub playback: Vec<SettingItem>,
    /// Library analysis: tracks measured, tracks there are to measure, and
    /// whether a pass is running now.
    pub analysed: i64,
    pub analysable: i64,
    pub analysing: bool,
    pub services: Vec<SettingItem>,
    /// TMDB, TheTVDB, OpenSubtitles, Last.fm and LibreTranslate, in the
    /// keychain.
    pub keys: Vec<ServiceKeyRow>,
    pub ai: Vec<SettingItem>,
    /// The "Requirements" sheet behind the AI panel: what each model asks of
    /// this machine. Built off the same manifest as `ai`, so the two lists
    /// can never disagree about which models exist.
    pub ai_requirements: Vec<SettingItem>,
    pub security: Vec<SettingItem>,
    pub data: Vec<SettingItem>,
    /// Newest first.
    pub backups: Vec<BackupRow>,
    pub data_path: String,
    pub advanced: Vec<SettingItem>,
    /// The long job running now (`maintenance::job`): a folder read again,
    /// the search index rebuilt, every folder rescanned. An empty key: none.
    /// `task_frac` is 0..1, or -1 where there is no honest fraction.
    pub task_key: String,
    pub task_label: String,
    pub task_frac: f64,
    /// What the last action did. Cleared by the next refresh.
    pub notice: String,
    // --- Sections ---
    /// Every section in sidebar order, with its state.
    pub sections: Vec<SectionRow>,
    /// The preset the current shape matches: an id from `section_presets`,
    /// or "custom".
    pub sections_preset: String,
    /// Every preset, built-in first, then the saved ones.
    pub section_presets: Vec<SectionsPresetRow>,
    /// The preset picked last, even if changed since. Empty if none.
    pub sections_last_preset: String,
    /// Whether the last preset can be taken back.
    pub sections_can_undo: bool,
    /// The sidebar's groups, in order.
    pub section_groups: Vec<SectionsGroup>,
    /// "cards" | "list" — how the panel draws the sections.
    pub sections_view: String,
    /// When the sidebar does not fit: "shrink" | "scroll" | "more".
    pub sidebar_overflow: String,
    /// Whether the sidebar draws the groups at all.
    pub sidebar_dividers: bool,
    /// The section that opens at launch, already resolved — if the saved choice
    /// has been hidden this is the fallback, not the choice.
    pub landing: String,
}

/// One preset, as the strip at the top of the Sections panel draws it.
pub struct SectionsPresetRow {
    /// A built-in id, or `mine:<name>` for a saved one.
    pub id: String,
    pub name: String,
    pub hint: String,
    /// The applications it shows. Home and Settings always are.
    pub ids: Vec<String>,
    pub mine: bool,
}

/// One sidebar group: a name and the section it starts at ("" for an empty
/// group at the end).
pub struct SectionsGroup {
    pub name: String,
    pub anchor: String,
}

/// One section, as the Sections panel draws it.
///
/// The label, the icon and the accent are the front end's — Dart already has
/// them in `kSectionMeta` and duplicating them here is how two lists start
/// disagreeing. What crosses is the id, the state, and the facts only this side
/// can measure.
pub struct SectionRow {
    /// `tulipix_core::sections::ALL` — a storage format, not a display string.
    pub id: String,
    /// "shown" | "hidden" | "off".
    pub mode: String,
    /// Whether it can be switched at all. Settings is the way back.
    pub locked: bool,
    /// The database files this section owns, comma-separated, or "" for one
    /// that owns none.
    pub db: String,
    /// What those files take up right now. Reported so the panel can say, where
    /// it is visible, that turning a section off deletes none of it.
    pub bytes: i64,
    /// Tabs inside the section that are switched off, by the id its page knows
    /// them by. Which tabs exist is the front end's — only Dart has the list —
    /// so this is the ones that were taken away, not the ones that are left.
    pub tabs_off: Vec<String>,
}

pub enum SettingsCmd {
    Refresh,
    SetTab { tab: String },
    Toggle { key: String, on: bool },
    SetText { key: String, value: String },
    /// A button row. The key names what to do; unknown keys are ignored rather
    /// than crashing a settings page.
    Action { key: String },
    /// `avatar` / `cover`: None leaves the picture alone, empty bytes remove
    /// it, anything else is a PNG the page already cropped.
    SaveProfile { name: String, emoji: String, logo: i32, avatar: Option<Vec<u8>>, cover: Option<Vec<u8>> },
    SetTheme { theme: String },
    SetReduceMotion { on: bool },
    SetHomeLayout { layout: String },
    HomeCardSet { key: String, on: bool },
    HomeCardsReset,
    LibAdd { path: String },
    LibRemove { path: String },
    /// "shown" | "hidden" | "off" for one section.
    SectionSet { id: String, mode: String },
    /// The sidebar order, front to back. Settings is dropped whatever position
    /// it arrives in — it is pinned last.
    SectionOrder { ids: Vec<String> },
    /// A preset id from `section_presets`.
    SectionPreset { name: String },
    /// Save what is shown now as a preset of this name.
    SectionPresetSave { name: String },
    SectionPresetDelete { name: String },
    /// Put back what the last preset replaced.
    SectionUndo,
    /// The sidebar order and its groups (`name=anchor`) together, since a drag
    /// across a group edge changes both.
    SectionLayout { ids: Vec<String>, groups: Vec<String> },
    /// Back to the default order and groups.
    SectionLayoutReset,
    /// Which section opens at launch.
    SectionLanding { id: String },
    /// One tab inside one section. `enabled: false` takes it out of that page's
    /// tab row; the page keeps whichever tab you are standing on until you leave
    /// it, so the row can never go blank under you.
    ///
    /// Not `on`: that is a Dart keyword, and frb renames it to `on_` rather than
    /// refusing — a field the Rust calls one thing and the Dart another.
    SectionTab { id: String, tab: String, enabled: bool },
}

pub async fn settings_dispatch(cmd: SettingsCmd) -> Result<SettingsState> {
    let mut notice = String::new();
    match cmd {
        SettingsCmd::Refresh => {}
        SettingsCmd::SetTab { tab } => set_tab(tab),
        SettingsCmd::Toggle { key, on } => {
            let mut s = load();
            s.flags.insert(key, on);
            save(s);
        }
        SettingsCmd::SetText { key, value } => match key.as_str() {
            // Two settings are real fields on the struct rather than map
            // entries, and writing them into `advanced` would store a key
            // nothing reads.
            "idle_lock_secs" => {
                let mut s = load();
                s.idle_lock_secs = value.trim().parse().unwrap_or(0);
                save(s);
            }
            // The "Lock after" choice: a label, stored as the seconds it means.
            "lock.after" => {
                if let Some((_, secs)) = LOCK_AFTER.iter().find(|(l, _)| *l == value) {
                    let mut s = load();
                    s.idle_lock_secs = *secs;
                    save(s);
                }
            }
            // Never stored as text: the core salts and hashes it.
            "lock.pin" => {
                let pin = value.trim();
                notice = if (4..=8).contains(&pin.len()) && pin.chars().all(|c| c.is_ascii_digit()) {
                    tulipix_core::account::set_pin(pin);
                    crate::api::lock::remember_pin_len(pin.len());
                    "PIN set. The lock screen asks for it from now on.".into()
                } else {
                    "A PIN is four to eight digits.".into()
                };
            }
            // A service key: the system keychain, never settings.json.
            k if k.starts_with("key:") => {
                notice = crate::api::maintenance::store_key(&k["key:".len()..], &value);
            }
            // Applied at once, as the Slint build does: the resolver caches the
            // folder, and a change it never heard of waits for a restart.
            // Blank (Reset) falls back to bundled and PATH.
            "tools.bin-dir" => {
                put(&key, &value);
                tulipix_core::thumbs::set_tool_dir(Some(&value));
            }
            _ => put(&key, &value),
        },
        SettingsCmd::Action { key } => notice = action(&key).await,
        SettingsCmd::SaveProfile { name, emoji, logo, avatar, cover } => {
            crate::api::shell::shell_dispatch(crate::api::shell::ShellCmd::SaveProfile {
                name,
                emoji,
                logo,
            })
            .await?;
            if let Some(png) = avatar {
                crate::api::shell::store_picture("avatar.png", &png)?;
            }
            if let Some(png) = cover {
                crate::api::shell::store_picture("cover.png", &png)?;
            }
        }
        SettingsCmd::SetTheme { theme } => {
            let mut s = load();
            s.theme = theme;
            save(s);
        }
        SettingsCmd::SetReduceMotion { on } => {
            let mut s = load();
            s.reduce_motion = on;
            save(s);
        }
        SettingsCmd::SetHomeLayout { layout } => put(HOME_LAYOUT_KEY, &layout),
        SettingsCmd::HomeCardSet { key, on } => {
            let mut s = load();
            s.flags.insert(format!("{HOME_CARD_PREFIX}{key}"), on);
            save(s);
        }
        SettingsCmd::HomeCardsReset => {
            let mut s = load();
            s.flags.retain(|k, _| !k.starts_with(HOME_CARD_PREFIX));
            save(s);
        }
        SettingsCmd::LibAdd { path } => {
            notice = match add_watched(Path::new(&path)) {
                true => format!("Watching {path}."),
                false => format!("{path} is already watched, or is not a folder."),
            };
        }
        SettingsCmd::LibRemove { path } => {
            remove_watched(Path::new(&path));
            notice = format!("Stopped watching {path}.");
        }
        SettingsCmd::SectionSet { id, mode } => {
            let mut s = load();
            tulipix_core::sections::set_mode(
                &mut s,
                &id,
                tulipix_core::sections::Mode::parse(&mode),
            );
            // Switching off the section the app opens on would leave it opening
            // onto nothing; `landing` falls back on read, and re-writing it here
            // is what makes the panel show the fallback rather than the choice
            // that no longer applies.
            let land = tulipix_core::sections::landing(&s).to_string();
            tulipix_core::sections::set_landing(&mut s, &land);
            save(s);
            notice = "Sidebar updated.".into();
        }
        SettingsCmd::SectionOrder { ids } => {
            let mut s = load();
            tulipix_core::sections::set_order(&mut s, &ids);
            save(s);
        }
        SettingsCmd::SectionPreset { name } => {
            let mut s = load();
            if tulipix_core::sections::apply_preset(&mut s, &name) {
                save(s);
                notice = "Sidebar updated.".into();
            }
        }
        SettingsCmd::SectionPresetSave { name } => {
            let mut s = load();
            if tulipix_core::sections::save_mine(&mut s, &name) {
                save(s);
                notice = format!("Saved \u{201c}{}\u{201d} as a preset.", name.trim());
            }
        }
        SettingsCmd::SectionPresetDelete { name } => {
            let mut s = load();
            tulipix_core::sections::delete_mine(&mut s, &name);
            save(s);
        }
        SettingsCmd::SectionUndo => {
            let mut s = load();
            if tulipix_core::sections::undo(&mut s) {
                save(s);
                notice = "Put back the sections you had.".into();
            }
        }
        SettingsCmd::SectionLayout { ids, groups } => {
            let mut s = load();
            tulipix_core::sections::set_order(&mut s, &ids);
            tulipix_core::sections::set_groups(&mut s, &groups);
            save(s);
        }
        SettingsCmd::SectionLayoutReset => {
            let mut s = load();
            tulipix_core::sections::reset_layout(&mut s);
            save(s);
        }
        SettingsCmd::SectionLanding { id } => {
            let mut s = load();
            tulipix_core::sections::set_landing(&mut s, &id);
            save(s);
        }
        SettingsCmd::SectionTab { id, tab, enabled } => {
            let mut s = load();
            tulipix_core::sections::set_tab(&mut s, &id, &tab, enabled);
            save(s);
        }
    }
    let mut state = snapshot().await;
    // A detached job that finished since says so on the next snapshot.
    state.notice = if notice.is_empty() {
        crate::api::maintenance::take_done().unwrap_or_default()
    } else {
        notice
    };
    Ok(state)
}

// ── the snapshot ────────────────────────────────────────────────────────────

fn tab_cell() -> &'static std::sync::Mutex<String> {
    static T: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::sync::Mutex::new("profile".into()))
}

fn set_tab(v: String) {
    if let Ok(mut g) = tab_cell().lock() {
        *g = v;
    }
}

const HOME_LAYOUT_KEY: &str = "home.layout";
const HOME_CARD_PREFIX: &str = "home.card.";

/// Every card the classic Home layout can draw, in the order it draws them.
/// Every card any layout can switch off, and what Settings calls it.
///
/// This list IS the contract. Home reads the cards as a LIST of enabled keys,
/// where a key nobody emits reads as off, so a card missing from here never
/// draws. `library` (Focused's Recently Added shelf and Timeline's counter
/// table) and `hub` (Cinema's one-at-a-time carousel) were once missing, and
/// those blocks silently vanished.
const HOME_CARDS: [(&str, &str); 14] = [
    ("hero", "Greeting"),
    ("continue", "Continue"),
    ("library", "Recently Added"),
    ("hub", "My Hub"),
    ("player", "Music player"),
    ("quick", "Quick actions"),
    ("photos", "Photos"),
    ("videos", "Videos"),
    ("music", "Music"),
    ("books", "Books"),
    ("cloud", "Cloud"),
    ("tools", "Tools"),
    ("transfer", "Transfer"),
    ("finances", "Finances"),
];

/// The card keys Settings left switched on, for whichever Home layout is
/// drawing. Every card defaults to on: a fresh install shows the whole page,
/// and the switches are there to take things away.
pub(crate) fn enabled_cards(s: &tulipix_core::settings::Settings) -> Vec<String> {
    HOME_CARDS
        .iter()
        .filter(|(k, _)| s.flag(&format!("{HOME_CARD_PREFIX}{k}"), true))
        // A card for a section that is off opens nothing. Eight of the fourteen
        // card keys are section ids, so the check is the same string; the other
        // six (the greeting, Continue, the player) are Home's own and are not in
        // `sections::ALL`, which is why this reads as "if it names a section".
        .filter(|(k, _)| {
            !tulipix_core::sections::ALL.contains(k)
                || tulipix_core::sections::mode_of(s, *k).works()
        })
        .map(|(k, _)| (*k).to_string())
        .collect()
}

async fn snapshot() -> SettingsState {
    // Keys typed into an old text row sat in settings.json, where nothing
    // reads them; they move to the keychain, where the sections do.
    crate::api::maintenance::rescue_typed_keys();
    let s = load();
    let (analysed, analysable, analysing) = crate::api::music::analyse_progress().await;
    let (task_key, task_label, task_frac) = crate::api::maintenance::job();
    SettingsState {
        tab: tab_cell().lock().map(|g| g.clone()).unwrap_or_else(|_| "profile".into()),
        display_name: s.text("profile.name"),
        avatar_emoji: s.text("profile.emoji"),
        avatar_path: crate::api::shell::picture("avatar.png"),
        cover_path: crate::api::shell::picture("cover.png"),
        logo_choice: s.text("profile.logo").parse().unwrap_or(0),
        theme: s.theme.clone(),
        reduce_motion: s.reduce_motion,
        home_layout: match s.text(HOME_LAYOUT_KEY) {
            l if l.is_empty() => "classic".into(),
            l => l,
        },
        home_cards: HOME_CARDS
            .iter()
            .map(|(key, label)| HomeCardRow {
                key: (*key).to_string(),
                label: (*label).to_string(),
                on: s.flag(&format!("{HOME_CARD_PREFIX}{key}"), true),
            })
            .collect(),
        home_music_left: s.flag("home.music-left", false),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        libraries: libraries(&s),
        default_cadence: match s.text("scan.cadence") {
            c if c.is_empty() => "manual".into(),
            c => c,
        },
        exclusions: s.text("scan.exclude"),
        playback: playback(&s),
        analysed,
        analysable,
        analysing,
        services: services(&s),
        keys: crate::api::maintenance::service_keys(),
        ai: ai(&s),
        ai_requirements: ai_requirement_rows(),
        security: security(&s),
        data: data(&s),
        backups: backups(),
        data_path: tulipix_core::paths::data_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        advanced: advanced(&s),
        task_key,
        task_label,
        task_frac,
        notice: String::new(),
        sections: section_rows(&s),
        sections_preset: tulipix_core::sections::current_preset(&s),
        section_presets: preset_rows(&s),
        sections_last_preset: tulipix_core::sections::last_preset(&s),
        sections_can_undo: tulipix_core::sections::can_undo(&s),
        section_groups: tulipix_core::sections::groups(&s)
            .into_iter()
            .map(|(name, anchor)| SectionsGroup { name, anchor })
            .collect(),
        sections_view: match s.text("sections.view").as_str() {
            "list" => "list".into(),
            _ => "cards".into(),
        },
        sidebar_overflow: sidebar_overflow(&s),
        sidebar_dividers: s.flag("sidebar.dividers", true),
        landing: tulipix_core::sections::landing(&s).into(),
    }
}

/// The databases each section owns. Named here rather than asked of each
/// section crate: this is the one place that has to know, and a section that is
/// off cannot be asked anything.
fn section_dbs(id: &str) -> &'static [&'static str] {
    match id {
        "photos" => &["photos.db"],
        "videos" => &["videos.db"],
        "music" => &["music.db", "podcasts.db"],
        "books" => &["books.db"],
        "cloud" => &["cloud.db"],
        "tools" => &["tools.db"],
        "transfer" => &["transfers.db"],
        "finances" => &["finances.db"],
        "feeds" => &["feeds.db"],
        "journal" => &["journal.db"],
        "kitchen" => &["kitchen.db"],
        "papers" => &["papers.db"],
        "voice" => &["voice.db"],
        "places" => &["places.db"],
        "studio" => &["studio.db"],
        "archive" => &["archive.db"],
        "arcade" => &["arcade.db"],
        // Home reads the other sections and Settings is a JSON file.
        _ => &[],
    }
}

fn preset_rows(s: &tulipix_core::settings::Settings) -> Vec<SectionsPresetRow> {
    let own = |v: &[&str]| -> Vec<String> { v.iter().map(|i| i.to_string()).collect() };
    tulipix_core::sections::PRESETS
        .iter()
        .map(|p| SectionsPresetRow {
            id: p.id.into(),
            name: p.name.into(),
            hint: p.hint.into(),
            ids: own(p.ids),
            mine: false,
        })
        .chain(tulipix_core::sections::mine(s).into_iter().map(|(name, ids)| {
            SectionsPresetRow {
                id: format!("{}{name}", tulipix_core::sections::MINE),
                name,
                hint: "yours".into(),
                ids,
                mine: true,
            }
        }))
        .collect()
}

/// `sidebar.overflow`, with anything unknown read as the default.
pub(crate) fn sidebar_overflow(s: &tulipix_core::settings::Settings) -> String {
    match s.text("sidebar.overflow").as_str() {
        v @ ("scroll" | "more") => v.into(),
        _ => "shrink".into(),
    }
}

fn section_rows(s: &tulipix_core::settings::Settings) -> Vec<SectionRow> {
    let dir = tulipix_core::paths::data_dir();
    tulipix_core::sections::ordered(s)
        .into_iter()
        .map(|(id, mode)| {
            let files = section_dbs(id);
            let bytes = dir
                .as_ref()
                .map(|d| {
                    files
                        .iter()
                        .filter_map(|f| std::fs::metadata(d.join(f)).ok())
                        .map(|m| m.len() as i64)
                        .sum::<i64>()
                })
                .unwrap_or(0);
            SectionRow {
                id: id.to_string(),
                mode: mode.key().to_string(),
                locked: tulipix_core::sections::ALWAYS.contains(&id),
                db: files.join(" · "),
                bytes,
                tabs_off: tulipix_core::sections::tabs_off(s, id),
            }
        })
        .collect()
}

// ── the panels ──────────────────────────────────────────────────────────────
//
// Transcribed from the Slint build's `wire_settings_panels`, key for key. The
// helpers below are the same four it uses, so a row reads the same on both
// sides and a copy edit is a one-line diff rather than a rewrite.

type S = tulipix_core::settings::Settings;

fn si(key: &str, kind: &str, label: &str, desc: &str, value: &str, on: bool, state: &str) -> SettingItem {
    SettingItem {
        key: key.into(),
        kind: kind.into(),
        label: label.into(),
        desc: desc.into(),
        value: value.into(),
        on,
        state: state.into(),
        options: Vec::new(),
        btn: String::new(),
        frac: -1.0,
    }
}

fn hdr(label: &str) -> SettingItem {
    si("", "header", label, "", "", false, "")
}

fn tog(s: &S, key: &str, def: bool, label: &str, desc: &str) -> SettingItem {
    si(key, "toggle", label, desc, "", s.flag(key, def), "")
}

fn txt(s: &S, key: &str, label: &str, desc: &str) -> SettingItem {
    si(key, "text", label, desc, &s.text(key), false, "")
}

fn stat(label: &str, value: &str, state: &str) -> SettingItem {
    si("", "status", label, "", value, false, state)
}

fn act(key: &str, label: &str, desc: &str, btn: &str) -> SettingItem {
    si(key, "action", label, desc, btn, false, "")
}

/// A status reading and its action button in one row. `value` is the reading
/// ("Installed", "Downloading"), `btn` the button ("Download", "Verify").
fn statact(key: &str, label: &str, desc: &str, value: &str, state: &str, btn: &str) -> SettingItem {
    let mut r = si(key, "status-action", label, desc, value, false, state);
    r.btn = btn.into();
    r
}

fn choice(s: &S, key: &str, label: &str, desc: &str, options: &[&str]) -> SettingItem {
    let mut row = si(key, "choice", label, desc, &s.text(key), false, "");
    row.options = options.iter().map(|o| (*o).to_string()).collect();
    if row.value.is_empty() {
        row.value = options.first().map(|o| (*o).to_string()).unwrap_or_default();
    }
    row
}

fn playback(s: &S) -> Vec<SettingItem> {
    vec![
        hdr("SUBTITLES"),
        txt(s, "playback.sub-size", "Subtitle size", "In pixels — 28 if left blank"),
        txt(s, "playback.sub-color", "Subtitle colour", "A colour code like #ffffff — applies on the next play"),
        stat("Subtitles next to the video", "Loaded automatically (.srt / .vtt / .ass)", "ok"),
        hdr("VIDEO"),
        tog(s, "playback.interpolation", false, "Smoother motion", "Frame interpolation — can be heavy on laptop graphics"),
        tog(s, "playback.upscale", false, "Upscale shaders (Anime4K)", "Sharper upscaling — drop .glsl shader files in the folder below"),
        shader_row(),
        stat("Keep display awake", "While a video plays", "ok"),
        hdr("AUDIO & MUSIC"),
        tog(s, "playback.audio-exclusive", false, "Exclusive audio output", "Bit-perfect output straight to the audio device — silences other apps"),
        txt(s, "music.eq-preset", "Music equalizer preset", "flat · rock · pop · jazz · bass · treble — applies on the next track"),
        tog(s, "music.autoplay", false, "Keep playing when the queue ends",
            "Builds a run from the last few tracks — their artists, genres, tempo and key — drawn only from your own library. Needs the analysis below to have run"),
        hdr("LIBRARY ANALYSIS"),
        act("music-analyse", "Measure tempo, key and dynamic range",
            "Reads every track once to work out its BPM, musical key, how compressed it is, and what it sounds like — which is what song matching, duplicate detection and keep-playing all read.              Runs in the background and can be stopped; already-measured tracks are skipped",
            "Analyse"),
        act("music-analyse-stop", "Stop measuring",
            "Finishes the track it is on and leaves the rest for next time", "Stop"),
    ]
}

/// What is in the upscale shader folder (`vmpv::glsl_chain` reads it), so an
/// empty folder with Upscale on is a reading rather than a mystery.
fn shader_row() -> SettingItem {
    let n = tulipix_core::paths::config_dir()
        .and_then(|d| std::fs::read_dir(d.join("shaders")).ok())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("glsl")))
                .count()
        })
        .unwrap_or(0);
    match n {
        0 => stat("Upscale shader folder", "Empty. Put Anime4K .glsl files in it", "muted"),
        1 => stat("Upscale shader folder", "1 shader in it", "ok"),
        n => stat("Upscale shader folder", &format!("{n} shaders in it"), "ok"),
    }
}

fn services(s: &S) -> Vec<SettingItem> {
    let mut rows = vec![
        hdr("OPTIONAL SERVICE KEYS"),
        // No key is a text row here. Every one of them is a secret, and this
        // file is settings.json: they are all keychain rows on the Service
        // keys list above (`keys`), which is where the sections read them
        // from. A key typed into a text row here went nowhere at all --
        // that was the TMDB bug, and Spotify and YouTube had it too.
        txt(s, "api.piped-instance", "YouTube backend server (Piped)", "The server used to browse YouTube. Leave blank for the default"),
        hdr("EXTRA METADATA SOURCES"),
        // Each of these is read only when it is switched on and its key is
        // saved, and none of them is ever the first source: they answer where
        // MusicBrainz, TMDB, OpenSubtitles or yt-dlp came back with nothing.
        tog(s, "api.discogs", false, "Discogs", "Artist biographies and metadata where MusicBrainz has no match. Needs a Discogs token above"),
        tog(s, "api.spotify", false, "Spotify", "Genres and artist pictures, from the Spotify catalogue. Needs the client ID and secret above"),
        tog(s, "api.youtube-data", false, "YouTube Data API", "View counts, upload dates and real thumbnails on the YouTube tab, in place of a yt-dlp subprocess per search. Needs a key above"),
        tog(s, "api.anilist", false, "AniList", "Anime titles, overviews and posters where TMDB has the wrong show. No key needed"),
        tog(s, "api.anidb", false, "AniDB", "Episode titles for anime, which AniList does not carry. Needs a client name registered at anidb.net"),
        tog(s, "api.addic7ed", false, "Addic7ed", "A second subtitle source, listed under whatever OpenSubtitles had. Best for television, often the same night. No key needed"),
        tog(s, "api.trakt", false, "Trakt.tv", "Send what you finish watching to your Trakt account. Needs the client ID and secret above, and Link below"),
        tog(s, "api.listenbrainz", false, "ListenBrainz", "Scrobble played music — the open Last.fm alternative"),
        hdr("SELF-HOSTED SERVERS (ADVANCED)"),
        // Not here: the update server, the crash-report server and the
        // place-name server. This build has no app updater, no crash
        // uploader and no place-name lookup to point at them, so a box for
        // each was an address nothing would ever use.
        txt(s, "api.radio-browser", "Radio station server", "Mirror for the internet-radio directory"),
        txt(s, "api.autoeq", "Headphone EQ database", "Mirror for AutoEq headphone profiles"),
        txt(s, "api.tmdb-image-base", "Poster artwork server", "Mirror for movie and show artwork"),
    ];
    rows.extend(trakt_rows(s));
    rows
}

/// Trakt is the one integration with a sign-in. Its client ID and secret are
/// keychain rows like any other, but the token behind them comes from the
/// device flow: Link asks Trakt for a code, shows it, and waits while the user
/// types it into trakt.tv/activate in their browser.
fn trakt_rows(s: &S) -> Vec<SettingItem> {
    if !s.flag("api.trakt", false) {
        return Vec::new();
    }
    let (value, state, btn) = match crate::api::maintenance::trakt_state() {
        crate::api::maintenance::TraktState::NoApp => (
            "Add the client ID and secret above".to_string(),
            "muted",
            "",
        ),
        crate::api::maintenance::TraktState::Waiting { code, url } => {
            (format!("Type {code} at {url}"), "busy", "Cancel")
        }
        crate::api::maintenance::TraktState::Linked(user) => {
            (format!("Linked as {user}"), "ok", "Unlink")
        }
        crate::api::maintenance::TraktState::NotLinked => {
            ("Not linked".to_string(), "warn", "Link")
        }
    };
    let mut row = statact(
        "trakt-link",
        "Trakt account",
        "What you finish watching is added to your Trakt history. Marking \
         something watched sends it too; un-marking it does not take it back",
        &value,
        state,
        btn,
    );
    if btn.is_empty() {
        row.kind = "status".into();
    }
    vec![hdr("TRAKT"), row]
}

fn ai(s: &S) -> Vec<SettingItem> {
    // Manifest-driven: ONE row per model -- status dot and Download/Verify
    // button together. Two separate rows is what the Slint build started with,
    // and the pair drifted the first time a model was renamed.
    let mut rows = vec![hdr("ON-DEVICE MODELS")];
    for m in &ai_manifest().models {
        let installed = tulipix_photos::ai::models::is_installed(m);
        let mb = m.size_bytes / 1_000_000;
        let (nice, purpose) = model_display(&m.name, &m.cap);
        let dl = ai_dl_progress().lock().ok().and_then(|g| g.get(m.name.as_str()).copied());
        let mut row = statact(
            &format!("ai-dl-{}", m.name),
            &format!("{nice} · {mb} MB"),
            purpose,
            if dl.is_some() { "Downloading" } else if installed { "Installed" } else { "Not downloaded" },
            if dl.is_some() { "busy" } else if installed { "ok" } else { "muted" },
            if installed { "Verify" } else { "Download" },
        );
        if let Some(f) = dl {
            row.frac = f as f64;
        }
        rows.push(row);
    }
    {
        let mut chk = act("ai-update-check", "Check for model updates",
            "Compare installed models against the latest versions", "Check");
        if AI_CHECK_BUSY.load(std::sync::atomic::Ordering::Relaxed) {
            chk.state = "busy".into();
        }
        rows.push(chk);
    }
    rows.extend([
        hdr("VOICE RECOGNITION"),
        choice(s, "ai.voice-lang", "Spoken language",
            "What the mic listens for — pinning a language beats auto-detect on short clips",
            &["en", "hi", "pa", "de", "fr", "es", "ru", "it", "auto"]),
        choice(s, "ai.model.voice", "Voice search",
            "Used by the mic button in search fields — Tiny answers fastest",
            &["tiny", "base", "small", "turbo"]),
        choice(s, "ai.model.transcribe", "Transcribe & subtitles",
            "Used by the Tools transcriber and video subtitles — bigger models catch more words",
            &["tiny", "base", "small", "turbo"]),
        stat("Model sizes", "Tiny bundled · Base 60 MB · Small 190 MB · Turbo 574 MB", "muted"),
        hdr("FEATURES"),
        tog(s, "ai.captions", false, "Describe photos automatically", "Writes captions and alt-text for new photos as they are added"),
        tog(s, "ai.voice", true, "Voice search", "The mic button in search bars — speak instead of typing"),
        tog(s, "ai.chat", false, "Chat assistant (Ctrl+J)", "Ask questions about your library in plain language"),
        hdr("BOOK READ-ALOUD"),
        tog(s, "books.tts.neural", true, "Use neural voice (Kokoro)",
            "Natural AI voice for the reader's Read Aloud. Off = robotic espeak voice (no model, needs espeak-ng)"),
        hdr("SCROBBLING"),
        tog(s, "api.listenbrainz", false, "Send listens to ListenBrainz",
            "The open listening record. Plays are queued while you are offline and sent when you are back"),
        txt(s, "api.listenbrainz-token", "ListenBrainz token",
            "From listenbrainz.org → Settings. Nothing is sent until this is filled in"),
        act("scrobble-flush", "Send queued listens now",
            "Plays are queued while you are offline and go out with the next one. This sends them immediately",
            "Send"),
        hdr("CLOUD"),
        tog(s, "ai.cloud-offload", false, "Allow cloud AI help", "Send selected questions to a cloud AI service. Off = everything stays on-device"),
    ]);
    rows
}

// ── AI model manifest ───────────────────────────────────────────────────────

/// Compiled-in AI model manifest (resources/ai-models.toml) -- parsed once.
/// The SAME file the Slint build reads, by `include_str!` rather than a copy:
/// two transcriptions of a model list is two lists that disagree.
fn ai_manifest() -> &'static tulipix_core::ai_models::Manifest {
    static M: std::sync::OnceLock<tulipix_core::ai_models::Manifest> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        tulipix_core::ai_models::Manifest::from_toml(include_str!(
            "../../../../resources/ai-models.toml"
        ))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "ai-models.toml parse");
            Default::default()
        })
    })
}

/// Human name + purpose for a manifest model, keyed off its capability gate so
/// the row says what the model DOES rather than naming its file.
fn model_display<'a>(name: &'a str, cap: &str) -> (&'a str, &'static str) {
    // By name first where one capability has two files: the face pair and
    // CLIP with its tokenizer read as the same model twice otherwise.
    match name {
        "face-rec-500m" => return ("Face recognition", "Tells faces apart so each person is grouped once"),
        "clip-vit-b32-tokenizer" => return ("Photo search words", "Turns what you type into something Photo search understands"),
        _ => {}
    }
    match cap {
        "photos.ai.faces" => ("Face detection", "Finds faces in photos so people can be grouped"),
        "photos.ai.clip" => ("Photo search", "Finds photos by what is in them — type “beach at sunset”"),
        "photos.ai.tags" => ("Object tags", "Names the things in your photos, for the Things tab"),
        "photos.ai.heal" => ("Magic eraser", "Removes unwanted objects in the photo editor"),
        "photos.ai.sky" => ("Smart select", "Selects sky / objects for one-tap edits"),
        "photos.ai.upscale" => ("Upscale", "Enlarges a photo four times in the editor, keeping detail"),
        "voice.balanced" => ("Whisper Base (balanced)", "Good accuracy at near-instant speed — best all-rounder"),
        "voice.accurate" => ("Whisper Small (accurate)", "Catches names and accents — great for subtitles"),
        "voice.best" => ("Whisper Turbo (best)", "Top accuracy for dictation-grade transcription"),
        "music.ai.embeddings" => ("Sounds like", "Finds songs that sound alike. Sonic Similar works without it"),
        "music.ai.stems" => ("Vocal remover", "Separates the voice from the music, for Karaoke and stems"),
        "books.ai.tts" => ("Natural voice", "Reads books aloud in a lifelike voice"),
        // A model the manifest gains before this list does still says which
        // section it serves, rather than showing no line at all.
        _ => (
            name,
            match cap.split('.').next() {
                Some("photos") => "Used by Photos",
                Some("music") => "Used by Music",
                Some("books") => "Used by Books",
                Some("voice") => "Used for speech",
                _ => "An on-device model",
            },
        ),
    }
}

/// "Check for model updates" in flight -- renders the button as a busy pill.
static AI_CHECK_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Live model-download progress (model name → 0..1). The download runs
/// detached, so this map is how a later `Refresh` learns how far it got --
/// there is no stream back to Dart and a download does not need one.
fn ai_dl_progress() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f32>>> =
        std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Per manifest entry: up-to-date, an older install awaiting update, or absent.
fn ai_update_summary() -> String {
    let (mut current, mut missing) = (0, 0);
    let mut stale: Vec<String> = Vec::new();
    let root = tulipix_photos::ai::models::models_root();
    for m in &ai_manifest().models {
        if tulipix_photos::ai::models::is_installed(m) {
            current += 1;
            continue;
        }
        // Any older "<name>-<version>" install dir → an update, not a gap.
        let older = root
            .as_deref()
            .and_then(|r| std::fs::read_dir(r).ok())
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{}-", m.name)));
        if older {
            stale.push(format!("{} → v{}", m.name, m.version));
        } else {
            missing += 1;
        }
    }
    if stale.is_empty() {
        format!("Models: {current} up-to-date · {missing} not installed — nothing to update.")
    } else {
        format!("Updates available: {} · {current} current · {missing} not installed.", stale.join(", "))
    }
}

/// Installed RAM in MB, or 0 where we cannot tell -- a machine that cannot be
/// measured gets a neutral row rather than a guess.
fn total_ram_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb / 1024)
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// One row per on-device model saying what it wants from this machine.
///
/// The RAM figure is a recommendation, not a measurement: weights have to be
/// resident and the runtime needs working buffers on top, which in practice
/// lands near three times the file -- floored at 512 MB so a tiny model does
/// not read as free.
fn ai_requirement_rows() -> Vec<SettingItem> {
    let ram = total_ram_mb();
    let mut rows = vec![hdr("THIS COMPUTER")];
    rows.push(stat(
        "Installed memory",
        &if ram > 0 { format!("{:.1} GB", ram as f64 / 1024.0) } else { "Unknown".to_string() },
        if ram == 0 { "muted" } else if ram >= 8192 { "ok" } else { "warn" },
    ));
    rows.push(stat(
        "Graphics acceleration",
        if cfg!(feature = "ai-onnx") {
            "ONNX runtime compiled in — GPU used when available"
        } else {
            "CPU only in this build"
        },
        if cfg!(feature = "ai-onnx") { "ok" } else { "muted" },
    ));

    rows.push(hdr("ON-DEVICE MODELS"));
    for m in &ai_manifest().models {
        let mb = m.size_bytes / 1_000_000;
        let need = (mb * 3).max(512);
        let (nice, purpose) = model_display(&m.name, &m.cap);
        // What the row asks for, in the order it matters: memory, then disk,
        // then whether a GPU is required or merely welcome.
        let gpu = if m.min_vram_mb > 0 {
            format!(" · {} MB VRAM", m.min_vram_mb)
        } else {
            String::new()
        };
        rows.push(si(
            "",
            "status",
            nice,
            purpose,
            &format!("{need} MB RAM · {mb} MB disk{gpu}"),
            false,
            if ram == 0 { "muted" } else if ram >= need { "ok" } else { "warn" },
        ));
    }

    rows.push(hdr("NOTES"));
    rows.push(stat("Only what you use is loaded", "Models load on demand and unload after", "muted"));
    rows.push(stat("Transcription is CPU-heavy", "Expect roughly real-time on four cores", "muted"));
    rows
}

/// "Lock after": each choice and the seconds it means.
const LOCK_AFTER: [(&str, u64); 7] = [
    ("1 minute", 60),
    ("2 minutes", 120),
    ("5 minutes", 300),
    ("10 minutes", 600),
    ("15 minutes", 900),
    ("30 minutes", 1800),
    ("1 hour", 3600),
];

/// The choice nearest `secs`, so a value typed into the old seconds box that
/// is not on the list still shows as something rather than as nothing.
fn lock_after_label(secs: u64) -> &'static str {
    LOCK_AFTER
        .iter()
        .min_by_key(|(_, s)| s.abs_diff(secs))
        .map(|(l, _)| *l)
        .unwrap_or("10 minutes")
}

fn security(s: &S) -> Vec<SettingItem> {
    let has_pin = tulipix_core::account::has_pin();
    let mut after = choice(s, "lock.after", "Lock after",
        "How long without a key press or the pointer moving. A playing video never locks",
        &LOCK_AFTER.map(|(l, _)| l));
    after.value = lock_after_label(crate::api::lock::idle_secs(s)).into();
    let mut rows = vec![
        hdr("LOCK"),
        tog(s, "autolock", false, "Lock when idle",
            "Shows the lock screen after a while without input, or now with Ctrl+L. Music keeps playing"),
        after,
        statact("lock-pin", "PIN",
            "Four to eight digits, asked for on the lock screen. Without one, a click unlocks",
            if has_pin { "Set" } else { "Not set" },
            if has_pin { "ok" } else { "muted" },
            if has_pin { "Change" } else { "Set a PIN" }),
    ];
    if has_pin {
        rows.push(act("lock-pin-clear", "Remove the PIN",
            "The lock screen goes back to unlocking with a click", "Remove"));
    }
    rows.extend([
        hdr("ON THE LOCK SCREEN"),
        tog(s, "lock.show-music", true, "What's playing", "Artwork, title and artist, lit in the cover's colours"),
        tog(s, "lock.media-controls", true, "Music controls without unlocking", "Play, pause, skip and love from the lock screen"),
        tog(s, "lock.show-lyrics", true, "Lyrics", "The synced line at the playhead, when the track has them"),
        tog(s, "lock.show-video", true, "A paused video", "Its frame and title. Resuming asks for the PIN"),
        tog(s, "lock.show-glance", true, "Glance cards", "Continue watching, bills due (a count, never amounts) and the library, when nothing is playing"),
        tog(s, "lock.motion", true, "Moving smoke",
            "A GPU shader drawn at a third of the window's size, twenty frames a second. Off: one still frame"),
        txt(s, "lock.wallpapers", "Wallpapers folder",
            "Pictures for when nothing is playing — the first ten, one every twelve seconds. Blank = the gradient"),
        act("lock-wallpapers-browse", "Choose the wallpapers folder", "Opens a folder picker", "Choose"),
        hdr("UNLOCK"),
        tog(s, "passkey", false, "Unlock with a passkey",
            "Unlock with your fingerprint or a security key instead of typing the PIN. The PIN \
             still works: this is another way in, never the only one"),
    ]);
    rows.extend(passkey_rows(s));
    rows.push(hdr("ENCRYPTION"));
    rows.extend(encryption_rows(s));
    rows.push({
        let (label, state) = sandbox();
        stat("OS sandbox", label, state)
    });
    rows
}

/// The encryption switch, and what it is actually doing.
///
/// Three things the old one-line toggle did not say: whether this build can
/// encrypt at all, where the key is kept, and that a flip does not happen
/// until the next start — which is the only moment nothing holds the
/// databases open.
fn encryption_rows(s: &S) -> Vec<SettingItem> {
    use tulipix_core::sec::db_encrypt::{self, State};
    if !db_encrypt::supported() {
        return vec![
            stat("Encrypt the library database", db_encrypt::why_not(), "muted"),
        ];
    }
    let mut rows = vec![tog(
        s,
        db_encrypt::FLAG,
        false,
        "Encrypt the library database",
        "SQLCipher over what Tulipix knows about your files — where they are, what you did with \
         them. Your files themselves are never touched. The key is 32 random bytes in the system \
         keychain, so the library opens when you log in and is unreadable on a stolen disk",
    )];
    rows.push(match db_encrypt::state() {
        State::Unsupported => stat("Encryption", db_encrypt::why_not(), "muted"),
        State::Off => stat("Encryption", "Off — the index is plain SQLite", "muted"),
        State::On => stat("Encryption", "On — every library database is encrypted", "ok"),
        State::PendingOn => stat(
            "Encryption",
            "Set to encrypt. It happens the next time Tulipix starts, before anything opens \
             the databases. Back up first",
            "warn",
        ),
        State::PendingOff => stat(
            "Encryption",
            "Set to decrypt. It happens the next time Tulipix starts",
            "warn",
        ),
    });
    if s.flag(db_encrypt::FLAG, false) {
        rows.push(act(
            "backup",
            "Back up before it happens",
            "Saves your settings and folder list. The databases themselves rebuild from a rescan, \
             which is the way back if a conversion ever goes wrong",
            "Back up",
        ));
    }
    rows
}

/// What is set up to unlock with, once the passkey switch is on.
///
/// Two back ends, and each says plainly when it is not there: fprintd is a
/// package, libfido2's tools are a package, and neither exists on macOS or
/// Windows in this build. A row that offered a button for something absent
/// would be the old switch all over again.
fn passkey_rows(s: &S) -> Vec<SettingItem> {
    use tulipix_core::sec::passkey::{self, PasskeyKind};
    if !s.flag("passkey", false) {
        return Vec::new();
    }
    let mut rows = Vec::new();

    // Fingerprint. Tulipix never enrols one — the fingers are the desktop's,
    // and this only turns them into a way into the app.
    let fp_on = passkey::has(PasskeyKind::Fingerprint);
    if !passkey::fingerprint::available() {
        rows.push(stat("Fingerprint", passkey::fingerprint::why_not(), "muted"));
    } else if !passkey::fingerprint::enrolled() {
        rows.push(stat(
            "Fingerprint",
            "No finger is enrolled with fprintd. Add one in your desktop's settings first",
            "warn",
        ));
    } else {
        rows.push(statact(
            if fp_on { "passkey-fp-remove" } else { "passkey-fp-add" },
            "Fingerprint",
            "The fingers already enrolled with fprintd. Nothing about them is stored by Tulipix",
            if fp_on { "On" } else { "Off" },
            if fp_on { "ok" } else { "muted" },
            if fp_on { "Turn off" } else { "Turn on" },
        ));
    }

    // Security keys, one row each, plus a row for whatever is plugged in and
    // not yet set up.
    if !passkey::security_key::available() {
        rows.push(stat("Security key", passkey::security_key::why_not(), "muted"));
        return rows;
    }
    let enrolled: Vec<_> = passkey::list()
        .into_iter()
        .filter(|c| c.kind == PasskeyKind::SecurityKey)
        .collect();
    for c in &enrolled {
        rows.push(statact(
            &format!("passkey-key-remove:{}", c.id),
            &c.label,
            "A security key set up to unlock Tulipix",
            "Set up",
            "ok",
            "Remove",
        ));
    }
    let plugged = passkey::security_key::devices();
    let new: Vec<_> = plugged
        .into_iter()
        .filter(|(_, label)| !enrolled.iter().any(|c| c.label == *label))
        .collect();
    if new.is_empty() && enrolled.is_empty() {
        rows.push(stat("Security key", "Plug one in to set it up", "muted"));
    }
    for (device, label) in new {
        rows.push(statact(
            &format!("passkey-key-add:{device}"),
            &label,
            "Plugged in. Setting it up asks you to touch it once",
            "Not set up",
            "muted",
            "Set up",
        ));
    }
    rows
}

/// Whether this copy runs inside an OS sandbox, read from what each one
/// leaves in the environment. A reading only: nothing here changes with it.
/// The Slint build printed a fixed "Not applicable on Linux".
fn sandbox() -> (&'static str, &'static str) {
    if cfg!(target_os = "linux") {
        if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
            ("Flatpak sandbox", "ok")
        } else if std::env::var_os("SNAP").is_some() {
            ("Snap sandbox", "ok")
        } else {
            ("None detected", "muted")
        }
    } else if cfg!(target_os = "macos") {
        if std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some() {
            ("App Sandbox", "ok")
        } else {
            ("None detected", "muted")
        }
    } else {
        ("None detected", "muted")
    }
}

fn data(s: &S) -> Vec<SettingItem> {
    let mut rows = vec![
        hdr("BACKUP"),
        act("backup", "Back up settings", "Saves your settings and folder list so you can restore them later", "Back up"),
        hdr("PROBLEMS"),
        act("open-logs", "Open the log folder", "tracing JSON logs with daily rotation", "Open"),
        act("open-data", "Open the data folder", "Where the section databases live", "Open"),
        tog(s, tulipix_core::multi_user::FLAG, false, "Profiles",
            "Several people sharing this computer account, each with their own library, settings \
             and PIN. Nothing is shared between them. Off, there is one library and no picker"),
    ];
    rows.extend(profile_rows(s));
    rows.extend(crashes());
    rows
}

/// The profiles on this computer, once the switch is on: one row each, with
/// Use or Delete, and the one in use marked.
///
/// A profile is a whole parallel set of folders, so switching is a restart —
/// the running process has its databases open, and pretending otherwise would
/// mean two libraries half-loaded in one window.
fn profile_rows(s: &S) -> Vec<SettingItem> {
    use tulipix_core::multi_user;
    if !s.flag(multi_user::FLAG, false) {
        return Vec::new();
    }
    let mut rows = vec![hdr("PROFILES")];
    for p in multi_user::list() {
        let key = if p.active {
            format!("profile-rename:{}", p.slug)
        } else {
            format!("profile-use:{}", p.slug)
        };
        rows.push(statact(
            &key,
            &p.display_name,
            &if p.active {
                "The profile this window is using. Rename takes the name from the box below"
                    .to_string()
            } else {
                format!("Its own library, in {}", tildeish(&p.data_dir))
            },
            if p.active { "In use" } else { "Ready" },
            if p.active { "ok" } else { "muted" },
            // The one in use has nothing to switch to, so its button is the
            // only other thing it can do: take the name from the box below.
            if p.active { "Rename" } else { "Use" },
        ));
        // Deleting the one in use, or the first one, is refused in the core;
        // not drawing the button is how that is said before it is pressed.
        if !p.active && p.slug != multi_user::DEFAULT_SLUG {
            rows.push(act(
                &format!("profile-delete:{}", p.slug),
                &format!("Delete {}", p.display_name),
                "Erases that profile's library index, settings and thumbnails. The files it \
                 pointed at are never touched",
                "Delete",
            ));
        }
    }
    rows.push(txt(s, PROFILE_NEW_KEY, "New profile",
        "A name, then Add. It starts empty: its own watched folders, its own settings, its own PIN"));
    rows.push(act("profile-add", "Add the profile", "Makes its folders and shows it above", "Add"));
    rows
}

/// A path with the home directory shortened, for a row that is describing
/// where something lives rather than naming a file to open.
pub(crate) fn tildeish(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

/// Where the name typed into the New profile box waits until Add is pressed.
const PROFILE_NEW_KEY: &str = "profile.new-name";

/// Crash reports, kept on this machine. One row each, newest first, with a
/// Report button that opens a pre-filled issue in the browser. Nothing is
/// uploaded and nothing is sent in the background: the row is the whole of it.
/// A machine that has never crashed gets one quiet status line instead of a
/// heading over nothing.
fn crashes() -> Vec<SettingItem> {
    let found = tulipix_core::crash::list();
    if found.is_empty() {
        return vec![
            hdr("CRASH REPORTS"),
            stat("Crash reports", "None on this computer", "ok"),
        ];
    }
    let mut rows = vec![
        hdr("CRASH REPORTS"),
        stat(
            "Kept here only",
            "Nothing is sent anywhere. Report opens a pre-filled issue in your browser",
            "muted",
        ),
    ];
    // Ten is the whole list anyone reads; the rest stay on disk and the folder
    // button is right there.
    for c in found.iter().take(10) {
        rows.push(statact(
            &format!("crash-report:{}", c.id),
            &crash_when(c.secs),
            &if c.location.is_empty() {
                c.headline.clone()
            } else {
                format!("{} — {}", c.headline, c.location)
            },
            &format!("Tulipix {}", c.version),
            "warn",
            "Report",
        ));
    }
    if found.len() > 10 {
        rows.push(stat("Older reports", &format!("{} more in the folder", found.len() - 10), "muted"));
    }
    rows.push(act("crash-open", "Open the crash folder", "The JSON files these rows are read from", "Open"));
    rows.push(act("crash-clear", "Delete the crash reports", "Removes every file in that folder", "Delete"));
    rows
}

/// "Today 14:05" / "3 Sep 2026 14:05" — a crash is looked up by when it
/// happened, so the clock time matters as much as the date.
fn crash_when(secs: i64) -> String {
    let Some(t) = chrono::DateTime::from_timestamp(secs, 0).map(|t| t.with_timezone(&chrono::Local))
    else {
        return "Unknown time".into();
    };
    use chrono::Datelike;
    let now = chrono::Local::now();
    if t.date_naive() == now.date_naive() {
        t.format("Today %H:%M").to_string()
    } else if t.year() == now.year() {
        t.format("%-d %b %H:%M").to_string()
    } else {
        t.format("%-d %b %Y %H:%M").to_string()
    }
}

/// Which display system the next launch uses, and which this one got.
///
/// X11 here means XWayland inside a Wayland session: the old protocol, which
/// still lets an app keep a window above the others and put one where it
/// wants -- the widget's pin, the tray panel under its icon. Wayland is the
/// sharper text at fractional scales and the future, and has neither.
fn display_rows(s: &S) -> Vec<SettingItem> {
    // The runner sets GDK_BACKEND from the setting before GTK starts, so the
    // environment is the answer for this session. No WAYLAND_DISPLAY at all
    // is a plain X11 session, where the choice makes no difference.
    let x11 = std::env::var("GDK_BACKEND").is_ok_and(|b| b.starts_with("x11"))
        || std::env::var_os("WAYLAND_DISPLAY").is_none();
    vec![
        choice(s, "ui.display-backend", "Display system",
            "Takes effect the next time Tulipix starts. X11 runs through XWayland and brings \
             back the widget's always-on-top pin and the tray panel opening under its icon; \
             it can look soft at fractional scaling. GDK_BACKEND, if you set it, wins",
            &["Automatic", "Wayland", "X11"]),
        stat("This session", if x11 { "X11" } else { "Wayland" }, "ok"),
    ]
}

fn advanced(s: &S) -> Vec<SettingItem> {
    let cache_mb = tulipix_core::thumbs::cache_size().map(|b| b / (1024 * 1024)).unwrap_or(0);
    let mut rows = vec![
        hdr("PERFORMANCE"),
        // The Rust side's. The Flutter engine and the Dart VM keep their own,
        // and startup, memory and slow frames are read on the Dart side.
        stat("Allocator", "System malloc", "ok"),
        tog(s, "power-aware", true, "Battery / network aware", "Hold automatic rescans back on a low battery"),
        // What that switch is reading right now.
        {
            use tulipix_core::power_aware::{battery_percent, power_source, PowerSource};
            match power_source() {
                PowerSource::Ac => stat("Power source", "On mains", "ok"),
                PowerSource::Battery => stat(
                    "Power source",
                    &match battery_percent() {
                        Some(p) => format!("On battery · {p} %"),
                        None => "On battery".into(),
                    },
                    "warn",
                ),
                PowerSource::Unknown => stat("Power source", "Not reported", "muted"),
            }
        },
        stat("Thumbnail cache", &format!("{cache_mb} MB"), "muted"),
        act("clear-thumbs", "Clear the thumbnail cache", "Every thumbnail is redrawn the next time it is needed", "Clear"),
        hdr("BUNDLED TOOLS"),
        txt(s, "tools.bin-dir", "Tools directory",
            "Folder holding yt-dlp, ffmpeg… — checked before bundled + PATH (blank = off)"),
        tool_row("ffmpeg"),
        tool_row("ffprobe"),
        tool_row("rclone"),
        tool_row("yt-dlp"),
        tog(s, tulipix_core::ytdlp::AUTO_UPDATE_FLAG, true, "Keep yt-dlp up to date",
            "Checks weekly. Sites change constantly and a yt-dlp a few weeks old \
             starts failing downloads with 403"),
        txt(s, "ytdlp.player-clients", "yt-dlp player clients",
            "Advanced, blank = yt-dlp's own defaults. Only set this if a yt-dlp \
             issue tells you to, e.g. default,tv,android"),
        tool_row("whisper-cli"),
        speech_model_row(),
        // Not a program to find: libmpv is loaded into the app by media_kit.
        // It ships inside the app on Windows and macOS; Linux links the
        // system's libmpv.
        stat("mpv", if cfg!(target_os = "linux") { "System library" } else { "Bundled" }, "ok"),
        tool_row("exiftool"),
        hdr("PLATFORM"),
        tog(s, "notifications", true, "Notifications", "When downloads and Tools jobs finish, and when bills are due"),
        // Read by the shell snapshot (`system_accent`, `follow_os_font_scale`).
        tog(s, "follow-system-accent", false, "Follow system accent", "Every section takes your desktop's accent colour"),
        tog(s, "follow-os-font-scale", true, "Honour OS font scale", "Text follows your desktop's text size; off keeps it at 100 %"),
        // Not here: the opt-in crash uploader. This build never sends a crash
        // anywhere -- the dumps are listed in Data › Crash reports and Report
        // opens an issue in the browser -- so a switch for it was a promise
        // nothing kept.
        // Readings only. The tray is a real probe; the other four say what this
        // build has, which on Linux is none of them. The window frame and the
        // OS sandbox are read elsewhere (Dart, and Security).
        stat(
            "System tray",
            if crate::shellsurface::tray_active() { "Active" } else { "No tray host" },
            if crate::shellsurface::tray_active() { "ok" } else { "warn" },
        ),
        stat("Share sheet", if cfg!(target_os = "linux") { "Not on Linux" } else { "Not in this build" }, "muted"),
        stat("Shortcuts (AppIntents)", if cfg!(target_os = "macos") { "Not in this build" } else { "Not on this OS" }, "muted"),
        stat("Desktop widgets", if cfg!(target_os = "linux") { "Not on Linux" } else { "Not in this build" }, "muted"),
        stat("Live Activities", if cfg!(target_os = "macos") { "Not in this build" } else { "Not on this OS" }, "muted"),
        hdr("MCP SERVER"),
        tog(s, tulipix_core::mcp::ENABLED_FLAG, false, "MCP server",
            "Lets an AI agent on this computer — Claude Desktop, an IDE assistant — search and \
             read your library. It runs as `tulipix-cli mcp`, started by the agent, not by \
             Tulipix: nothing listens on a port and nothing runs in the background"),
        hdr("BUILD"),
        stat("Renderer", "Flutter", "ok"),
        stat("Player embedding", "libmpv inside the app, through media_kit", "ok"),
        stat(
            "ONNX editor ops",
            if cfg!(feature = "ai-onnx") { "Compiled in" } else { "Not in this build" },
            if cfg!(feature = "ai-onnx") { "ok" } else { "muted" },
        ),
        stat("Audio formats", &tulipix_music::formats::FORMATS
            .iter()
            .map(|f| f.ext)
            .collect::<Vec<_>>()
            .join(" · "), "ok"),
        // The formats whose gapless playback has not been checked, the Slint
        // build's claim table's other half.
        stat("Gapless unverified", &{
            let g = tulipix_music::formats::gapless_gaps();
            if g.is_empty() { "None".to_string() } else { g.join(" · ") }
        }, "muted"),
        stat("Video formats", &tulipix_videos::scan::VIDEO_EXTS.join(" · "), "ok"),
        stat("Tools formats", &tools_formats(), "ok"),
        stat("Book formats", &book_formats(), "ok"),
    ];
    // Linux only: Windows and macOS have one display system and nothing to
    // pick. Read by the runner (`linux/runner/main.cc`) straight out of
    // settings.json before GTK starts -- which is why it is "next launch":
    // the backend is fixed the moment the first window opens.
    if cfg!(target_os = "linux") {
        if let Some(at) = rows.iter().position(|r| r.key == "follow-os-font-scale") {
            rows.splice(at + 1..at + 1, display_rows(s));
        }
    }
    // The MCP rows that only mean anything once the server is on go in beside
    // their switch, not at the end of the tab.
    if let Some(at) = rows.iter().position(|r| r.key == tulipix_core::mcp::ENABLED_FLAG) {
        let extra = mcp_rows(s);
        rows.splice(at + 1..at + 1, extra);
    }
    rows
}

/// The rows under the MCP switch, once it is on: what the agent may do, the
/// command to point it at, and the configuration block to paste.
fn mcp_rows(s: &S) -> Vec<SettingItem> {
    if !s.flag(tulipix_core::mcp::ENABLED_FLAG, false) {
        return Vec::new();
    }
    let reads = tulipix_core::mcp::BUILTIN_TOOLS
        .iter()
        .filter(|t| matches!(t.kind, tulipix_core::mcp::ToolKind::Read))
        .count();
    let writes = tulipix_core::mcp::BUILTIN_TOOLS.len() - reads;
    let writes_on = s.flag(tulipix_core::mcp::WRITE_FLAG, false);
    vec![
        tog(s, tulipix_core::mcp::WRITE_FLAG, false, "Let agents change things",
            "Off, an agent can read the library and nothing else. On, it can also tag and star \
             photos. The tools that write are not even listed to the agent while this is off"),
        stat(
            "Tools offered",
            &if writes_on {
                format!("{reads} that read · {writes} that change things")
            } else {
                format!("{reads} that read · {writes} held back")
            },
            if writes_on { "warn" } else { "ok" },
        ),
        stat("Reads", "Photos, videos, music and books. Never Finances", "muted"),
        stat("Command", &mcp_command(), "ok"),
        // The whole block, so it is one copy rather than four fields typed by
        // hand into a JSON file.
        stat("Claude Desktop config", &mcp_config_json(), "muted"),
    ]
}

/// Where `tulipix-cli` is, as an agent would have to spell it: the bundled
/// copy, else beside this executable, else the bare name for a copy on PATH.
fn mcp_command() -> String {
    let exe = if cfg!(target_os = "windows") { "tulipix-cli.exe" } else { "tulipix-cli" };
    if let Some(p) = tulipix_core::thumbs::bundled_file(exe) {
        return format!("{} mcp", p.display());
    }
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(exe)))
        .filter(|p| p.exists())
    {
        return format!("{} mcp", beside.display());
    }
    format!("{exe} mcp")
}

/// The `mcpServers` entry Claude Desktop wants, filled in for this computer.
fn mcp_config_json() -> String {
    let command = mcp_command();
    let program = command.trim_end_matches(" mcp");
    serde_json::json!({
        "mcpServers": {
            "tulipix": { "command": program, "args": ["mcp"] }
        }
    })
    .to_string()
}

/// Every "Convert to" and "Save as" choice in the Tools catalog -- its
/// `target_ext` and `format` picks -- in order and once each. Read from the
/// catalog, so a new conversion shows up here by itself.
fn tools_formats() -> String {
    let mut out: Vec<&str> = Vec::new();
    for op in tulipix_tools::catalog::CATALOG {
        for f in op.fields {
            if matches!(f.key, "target_ext" | "format") {
                for &o in f.options {
                    if !out.contains(&o) {
                        out.push(o);
                    }
                }
            }
        }
    }
    out.join(" · ")
}

/// What the Books section opens: each candidate put through `format_of`, the
/// function its scanner asks, so a format it stops accepting drops out here.
fn book_formats() -> String {
    ["epub", "pdf", "djvu", "cbz", "cbr", "cb7", "cbt", "fb2", "mobi", "azw3"]
        .into_iter()
        .filter(|e| tulipix_books::format_of(Path::new(&format!("x.{e}"))).is_some())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The model whisper-cli listens with, found as Tools › Transcribe finds it:
/// the first `.bin` beside the whisper-cli it runs.
fn speech_model_row() -> SettingItem {
    match crate::api::tools::whisper_model() {
        Some(p) => stat(
            "Speech model",
            &p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "ok",
        ),
        None => stat("Speech model", "Missing: put a ggml .bin beside whisper-cli", "warn"),
    }
}

/// Where one external tool is coming from. The order is the one the app
/// actually resolves in: the override folder, then the app's own updated copy,
/// then the bundled copy, then PATH -- so "System" is a warning, not a pass: a
/// system binary is whatever version happens to be installed.
///
/// yt-dlp additionally prints its version, because that is the number anyone
/// looking at this row is actually there to check: a download failing with 403
/// is nearly always a binary some weeks old, and "Bundled" never said that.
fn tool_row(name: &str) -> SettingItem {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let file = format!("{name}{ext}");
    let version = if name == "yt-dlp" {
        tulipix_core::ytdlp::installed_version()
    } else {
        None
    };
    let with_version = |source: &str| -> String {
        match &version {
            Some(v) => format!("{source} · {v}"),
            None => source.to_string(),
        }
    };
    let dir = load().text("tools.bin-dir");
    let dir = dir.trim();
    if !dir.is_empty() && Path::new(dir).join(&file).exists() {
        return stat(name, &with_version("Tools directory"), "ok");
    }
    if tulipix_core::ytdlp::managed_dir().is_some_and(|d| d.join(&file).exists()) {
        return stat(name, &with_version("Updated"), "ok");
    }
    if tulipix_core::thumbs::bundled_file(&file).is_some() {
        return stat(name, &with_version("Bundled"), "ok");
    }
    if on_path(&file) {
        return stat(name, &with_version("System"), "warn");
    }
    stat(name, "Missing", "error")
}

fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&paths).any(|d| d.join(name).exists())
}

// ── actions ─────────────────────────────────────────────────────────────────

async fn action(key: &str) -> String {
    match key {
        "backup" => match backup() {
            Ok(dir) => format!("Backed up to {}.", dir.display()),
            Err(e) => format!("Backup failed: {e}"),
        },
        "open-logs" => open(tulipix_core::paths::data_dir().map(|d| d.join("logs"))),
        "crash-open" => open(tulipix_core::crash::dir()),
        // Profiles. Switching is a restart, because this process has its
        // databases open and a profile is a different set of them.
        "profile-add" => {
            let name = load().text(PROFILE_NEW_KEY);
            let name = name.trim().to_string();
            if name.is_empty() {
                return "Type a name for the profile first.".into();
            }
            match tulipix_core::multi_user::create(&name) {
                Ok(_) => {
                    put(PROFILE_NEW_KEY, "");
                    format!("Added {name}. Press Use to open it.")
                }
                Err(e) => format!("Could not add it: {e}"),
            }
        }
        k if k.starts_with("profile-use:") => {
            let slug = &k["profile-use:".len()..];
            // Only a profile that is actually there: the key is text from the
            // page, and this is about to become an environment variable.
            if !tulipix_core::multi_user::list().iter().any(|p| p.slug == slug) {
                return "There is no profile by that name.".into();
            }
            relaunch_as_profile(slug);
            "Opening that profile — Tulipix is starting again.".into()
        }
        k if k.starts_with("profile-delete:") => {
            match tulipix_core::multi_user::delete(&k["profile-delete:".len()..]) {
                Ok(()) => "That profile is gone. The files it pointed at are untouched.".into(),
                Err(e) => format!("Could not delete it: {e}"),
            }
        }
        // The one in use: its name is the only thing that can change from
        // here, and it is typed in the New profile box.
        k if k.starts_with("profile-rename:") => {
            let slug = k["profile-rename:".len()..].to_string();
            let name = load().text(PROFILE_NEW_KEY);
            let name = name.trim().to_string();
            if name.is_empty() {
                return "Type the new name in the box below, then press Rename.".into();
            }
            match tulipix_core::multi_user::set_display_name(&slug, &name) {
                Ok(()) => {
                    put(PROFILE_NEW_KEY, "");
                    format!("This profile is called {name} now.")
                }
                Err(e) => format!("Could not rename it: {e}"),
            }
        }
        "crash-clear" => match tulipix_core::crash::clear() {
            0 => "There were no crash reports to delete.".into(),
            1 => "Deleted 1 crash report.".into(),
            n => format!("Deleted {n} crash reports."),
        },
        // The browser, with the issue form filled in. Nothing is posted: the
        // page opens and the user decides whether to send it.
        k if k.starts_with("crash-report:") => {
            match tulipix_core::crash::find(&k["crash-report:".len()..]) {
                Some(c) => {
                    crate::api::transfer::open_url(&tulipix_core::crash::issue_url(&c));
                    "Opened an issue with the details filled in. Nothing is sent until you post it.".into()
                }
                None => "That crash report is not there any more.".into(),
            }
        }
        "open-data" => open(tulipix_core::paths::data_dir()),
        // Where the upscale chain is read from (`vmpv::glsl_chain`).
        "open-shaders" => open(tulipix_core::paths::config_dir().map(|d| d.join("shaders"))),
        // Only a folder that is on the watched list: the key is text from the
        // page, and this opens whatever it names.
        k if k.starts_with("lib-open:") => {
            let dir = PathBuf::from(&k["lib-open:".len()..]);
            if watched().contains(&dir) { open(Some(dir)) } else { "That folder is not watched.".into() }
        }
        // Advanced › Bundled tools: yt-dlp updates itself; the rest open
        // their download page.
        k if k.starts_with("tool-update:") => {
            crate::api::maintenance::update_tool(&k["tool-update:".len()..])
        }
        k if k.starts_with("key-remove:") => {
            crate::api::maintenance::remove_key(&k["key-remove:".len()..])
        }
        k if k.starts_with("key-test:") => {
            crate::api::maintenance::test_key(&k["key-test:".len()..]).await
        }
        // One watched folder read again, or its thumbnails redrawn. Only a
        // folder on the watched list, as with lib-open.
        k if k.starts_with("lib-rescan:") => {
            let dir = PathBuf::from(&k["lib-rescan:".len()..]);
            if !watched().contains(&dir) {
                return "That folder is not watched.".into();
            }
            crate::api::maintenance::start_rescan(dir)
        }
        // What each folder holds, counted when the Libraries tab opens. Says
        // nothing: the figures on the cards are the answer.
        "lib-counts" => {
            crate::api::maintenance::refresh_counts().await;
            String::new()
        }
        k if k.starts_with("lib-thumbs:") => {
            let dir = PathBuf::from(&k["lib-thumbs:".len()..]);
            if !watched().contains(&dir) {
                return "That folder is not watched.".into();
            }
            match crate::api::maintenance::clear_folder_thumbs(&dir) {
                0 => "That folder had no thumbnails cached.".into(),
                n => format!(
                    "Cleared {n} thumbnail{}. They are drawn again as they come into view.",
                    if n == 1 { "" } else { "s" }
                ),
            }
        }
        "rebuild-search" => crate::api::maintenance::start_rebuild_search(),
        // Link, cancel or unlink, depending on where the device flow is.
        "trakt-link" => crate::api::maintenance::trakt_link_action(),
        // `lib-section:<key>:<folder>`. The key has no colon; the folder may
        // (C:\Music), so the split is at the first one.
        k if k.starts_with("lib-section:") => {
            let Some((sec, folder)) = k["lib-section:".len()..].split_once(':') else {
                return "Which folder?".into();
            };
            if !watched().iter().any(|w| Path::new(folder).starts_with(w)) {
                return "That folder is not watched.".into();
            }
            match crate::api::maintenance::set_music_section(folder, sec).await {
                Ok(label) => format!("Its music is in {label} now."),
                Err(e) => format!("Could not move it: {e}"),
            }
        }
        "export" => match crate::api::maintenance::export_library().await {
            Ok(file) => {
                open(file.parent().map(Path::to_path_buf));
                format!("Exported to {}.", file.display())
            }
            Err(e) => format!("Export failed: {e}"),
        },
        k if k.starts_with("import:") => {
            crate::api::maintenance::import_from(Path::new(&k["import:".len()..])).await
        }
        k if k.starts_with("backup-restore-") => {
            let id = &k["backup-restore-".len()..];
            match restore(id) {
                Ok(()) => "Restored. Your settings from before are a backup of their own now. \
                           Some changes show after a restart."
                    .into(),
                Err(e) => format!("Could not restore: {e}"),
            }
        }
        "music-analyse" => {
            let n = crate::api::music::music_analyse_all().await.unwrap_or(0);
            match n {
                0 => "Every track has already been measured.".into(),
                1 => "Measuring 1 track…".into(),
                n => format!("Measuring {n} tracks… you can keep using the app."),
            }
        }
        "music-analyse-stop" => {
            let _ = crate::api::music::music_analyse_stop().await;
            "Stopping after the current track.".into()
        }
        "scrobble-flush" => {
            let waiting = crate::api::scrobble::scrobble_pending_count()
                .await
                .unwrap_or(0);
            if waiting == 0 {
                return "Nothing is waiting to be sent.".into();
            }
            match crate::api::scrobble::scrobble_flush().await {
                Ok(0) => format!(
                    "{waiting} listen{} still waiting — check the token, or that you are online.",
                    if waiting == 1 { "" } else { "s" }
                ),
                Ok(n) => format!("Sent {n} listen{}.", if n == 1 { "" } else { "s" }),
                Err(e) => format!("Could not send: {e}"),
            }
        }
        // Fingerprint: nothing is enrolled here, only recorded as a way in.
        // The fingers are fprintd's, put there by the desktop's own settings.
        "passkey-fp-add" => {
            use tulipix_core::sec::passkey::{self, PasskeyCredential, PasskeyKind};
            if !passkey::fingerprint::enrolled() {
                return "No finger is enrolled with fprintd yet. Add one in your desktop's settings first.".into();
            }
            match passkey::add(PasskeyCredential {
                id: String::new(),
                kind: PasskeyKind::Fingerprint,
                label: std::env::var("USER").unwrap_or_else(|_| "this account".into()),
                public_key: String::new(),
                created_at: tulipix_core::util::unix_secs(),
            }) {
                Ok(()) => "The lock screen takes your fingerprint now. The PIN still works.".into(),
                Err(e) => format!("Could not turn it on: {e}"),
            }
        }
        "passkey-fp-remove" => {
            use tulipix_core::sec::passkey::{self, PasskeyKind};
            match passkey::remove("", PasskeyKind::Fingerprint) {
                Ok(()) => "The lock screen no longer asks for a fingerprint.".into(),
                Err(e) => format!("Could not turn it off: {e}"),
            }
        }
        // A security key. Blocking on both counts -- it waits for a touch --
        // so it goes to a thread that is allowed to wait.
        k if k.starts_with("passkey-key-add:") => {
            use tulipix_core::sec::passkey::{self, security_key};
            let device = k["passkey-key-add:".len()..].to_string();
            // Only a device the tools are reporting right now: the key is
            // text from the page, and this runs a program with it.
            let Some((device, label)) = security_key::devices()
                .into_iter()
                .find(|(d, _)| *d == device)
            else {
                return "That security key is not plugged in any more.".into();
            };
            let done = tokio::task::spawn_blocking(move || {
                security_key::enrol(&device, &label).and_then(passkey::add)
            })
            .await;
            match done {
                Ok(Ok(())) => "That security key unlocks Tulipix now. The PIN still works.".into(),
                Ok(Err(e)) => format!("Could not set it up: {e}"),
                Err(e) => format!("Could not set it up: {e}"),
            }
        }
        k if k.starts_with("passkey-key-remove:") => {
            use tulipix_core::sec::passkey::{self, PasskeyKind};
            let id = &k["passkey-key-remove:".len()..];
            match passkey::remove(id, PasskeyKind::SecurityKey) {
                Ok(()) => "That security key no longer unlocks Tulipix.".into(),
                Err(e) => format!("Could not remove it: {e}"),
            }
        }
        "lock-pin-clear" => {
            tulipix_core::account::set_pin("");
            crate::api::lock::remember_pin_len(0);
            "PIN removed. A click unlocks now.".into()
        }
        "clear-thumbs" => match tulipix_core::paths::thumbs_dir() {
            Some(dir) => {
                std::fs::remove_dir_all(&dir).ok();
                std::fs::create_dir_all(&dir).ok();
                "Thumbnail cache cleared.".into()
            }
            None => "No thumbnail cache to clear.".into(),
        },
        // Manifest vs what is on disk. Cheap enough to await inline -- it is
        // a directory listing, not a network call.
        "ai-update-check" => {
            AI_CHECK_BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
            let msg = ai_update_summary();
            AI_CHECK_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            msg
        }
        // Model download / verify. The download is detached and reports into
        // `ai_dl_progress`: a 574 MB blob cannot be awaited inside a settings
        // dispatch, and Dart polls `Refresh` while any row reads "busy".
        k if k.starts_with("ai-dl-") => {
            let name = k.trim_start_matches("ai-dl-").to_string();
            let Some(entry) = ai_manifest().find(&name).cloned() else {
                return format!("No model called “{name}” is in the manifest.");
            };
            // A second click while it is already running is a no-op, not a
            // second download writing over the same file.
            if ai_dl_progress().lock().map(|g| g.contains_key(&name)).unwrap_or(false) {
                return format!("{name} is already downloading.");
            }
            if let Ok(mut g) = ai_dl_progress().lock() {
                g.insert(name.clone(), 0.0);
            }
            let started = name.clone();
            tokio::spawn(async move {
                // Record every >=1% step; the poll reads whatever is current,
                // so finer granularity would only cost locks.
                let mut last = -1.0f32;
                let prog_name = name.clone();
                let res = tulipix_photos::ai::models::download_with_progress(&entry, move |f| {
                    if f - last >= 0.01 || f >= 1.0 {
                        last = f;
                        if let Ok(mut g) = ai_dl_progress().lock() {
                            g.insert(prog_name.clone(), f);
                        }
                    }
                })
                .await;
                // Cleared on both paths: a failed download left in the map
                // would stick the row on "Downloading" for the session.
                if let Ok(mut g) = ai_dl_progress().lock() {
                    g.remove(&name);
                }
                match res {
                    Ok(_) => tracing::info!(%name, "model installed"),
                    Err(e) => tracing::error!(%name, error = %e, "model download"),
                }
            });
            format!("Downloading {started}…")
        }
        // Remove: the model's whole folder, pinned digest included, so a later
        // Download starts clean. Dart has already asked.
        k if k.starts_with("ai-rm-") => {
            let name = k.trim_start_matches("ai-rm-");
            let Some(entry) = ai_manifest().find(name) else {
                return format!("No model called “{name}” is in the manifest.");
            };
            if ai_dl_progress().lock().map(|g| g.contains_key(name)).unwrap_or(false) {
                return format!("{name} is still downloading.");
            }
            let nice = model_display(&entry.name, &entry.cap).0;
            match tulipix_photos::ai::models::install_dir(entry) {
                Some(dir) if dir.exists() => match std::fs::remove_dir_all(&dir) {
                    Ok(()) => format!("Removed {nice}."),
                    Err(e) => format!("Could not remove {nice}: {e}"),
                },
                _ => format!("{nice} is not on this computer."),
            }
        }
        // An unknown key is a row that has a button and no handler yet. Saying
        // so beats a button that silently does nothing.
        other => format!("Nothing is wired to “{other}” yet."),
    }
}

/// Start a fresh copy in another profile and leave.
///
/// The same shape as the reset relaunch in `api::status`: a beat so the page
/// can say what is happening, then a new process with `TULIPIX_PROFILE` set,
/// then out. It has to be a new process — every path in the app is read from
/// `paths::profile_suffix`, which is read once, and this one's databases are
/// open.
pub(crate) fn relaunch_as_profile(slug: &str) {
    let value = if slug == tulipix_core::multi_user::DEFAULT_SLUG {
        String::new()
    } else {
        slug.to_string()
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .env(tulipix_core::paths::PROFILE_ENV, &value)
                .spawn();
        }
        std::process::exit(0);
    });
}

fn open(dir: Option<PathBuf>) -> String {
    let Some(dir) = dir else { return "That folder does not exist on this system.".into() };
    std::fs::create_dir_all(&dir).ok();
    match tulipix_platform::fm::open_default(&dir) {
        Ok(()) => format!("Opened {}.", dir.display()),
        Err(e) => format!("Could not open {}: {e}", dir.display()),
    }
}

/// Minimal backup: settings.json and watched_folders.json into a timestamped
/// folder under the data dir. The databases are not in it -- they are the part
/// a rescan can rebuild, and the part a copy could catch mid-write.
fn backup() -> Result<PathBuf> {
    let data = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?;
    let dest = data.join("backups").join(crate::api::home::now_secs().to_string());
    std::fs::create_dir_all(&dest)?;
    if let Some(cfg) = tulipix_core::paths::config_dir() {
        for f in ["settings.json", "watched_folders.json"] {
            let src = cfg.join(f);
            if src.exists() {
                std::fs::copy(&src, dest.join(f))?;
            }
        }
    }
    Ok(dest)
}

/// What `backup` has written, newest first. Folders not named by a number are
/// not backups and are left out.
fn backups() -> Vec<BackupRow> {
    let Some(root) = tulipix_core::paths::data_dir().map(|d| d.join("backups")) else {
        return Vec::new();
    };
    let mut out: Vec<BackupRow> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let secs = id.parse::<i64>().ok()?;
            let (mut files, mut bytes) = (0u32, 0u64);
            for f in std::fs::read_dir(e.path()).ok()?.flatten() {
                if let Ok(m) = f.metadata()
                    && m.is_file()
                {
                    files += 1;
                    bytes += m.len();
                }
            }
            Some(BackupRow { id, secs, files, bytes })
        })
        .collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs));
    out
}

/// Copy a backup's files back over the live ones. The live ones are backed up
/// first, so a restore is itself undoable.
fn restore(id: &str) -> Result<()> {
    // The id becomes a path, so it has to be the bare number `backup` names
    // its folders with -- never a `..` or a separator.
    anyhow::ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()), "that is not a backup");
    let data = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?;
    let src = data.join("backups").join(id);
    anyhow::ensure!(src.is_dir(), "that backup is no longer there");
    let cfg = tulipix_core::paths::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config directory on this system"))?;
    backup()?;
    std::fs::create_dir_all(&cfg)?;
    for f in ["settings.json", "watched_folders.json"] {
        let from = src.join(f);
        if from.exists() {
            std::fs::copy(&from, cfg.join(f))?;
        }
    }
    Ok(())
}

// ── watched folders ─────────────────────────────────────────────────────────

fn watched_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

pub(crate) fn watched() -> Vec<PathBuf> {
    let Some(p) = watched_path() else { return Vec::new() };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|b| serde_json::from_str::<Vec<String>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn write_watched(list: &[PathBuf]) -> bool {
    let Some(p) = watched_path() else { return false };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let out: Vec<String> = list.iter().map(|x| x.to_string_lossy().into_owned()).collect();
    match serde_json::to_string_pretty(&out) {
        Ok(body) => std::fs::write(&p, body).is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "settings: serialise watched folders");
            false
        }
    }
}

fn add_watched(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let mut list = watched();
    if list.iter().any(|x| x == dir) {
        return false;
    }
    list.push(dir.to_path_buf());
    write_watched(&list)
}

fn remove_watched(dir: &Path) {
    let mut list = watched();
    list.retain(|x| x != dir);
    write_watched(&list);
}

/// The watched list, with whether each folder is still on disk. Removing a
/// folder here does not delete anything indexed from it -- the next scan marks
/// those rows missing, which is reversible; a delete is not.
fn libraries(s: &S) -> Vec<LibraryRow> {
    let sections = tulipix_common::load_folder_sections();
    watched()
        .into_iter()
        .map(|p| {
            let path = p.to_string_lossy().into_owned();
            // Keys are stored without a trailing separator.
            let music_section = sections
                .get(path.trim_end_matches(['/', '\\']))
                .cloned()
                .unwrap_or_else(|| "mymusic".into());
            let items = crate::api::maintenance::folder_items(&path).unwrap_or(-1);
            let last_scan = s.text(&format!("lib.scanned.{path}")).parse().unwrap_or(0);
            let cadence = s.text(&format!("lib.cadence.{path}"));
            LibraryRow {
                exists: p.exists(),
                path,
                sections: Vec::new(),
                music_section,
                items,
                last_scan,
                cadence,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every panel opens with a header, or the first rows sit under nothing.
    #[test]
    fn every_panel_starts_with_a_header() {
        let s = S::default();
        for (name, rows) in [
            ("playback", playback(&s)),
            ("services", services(&s)),
            ("ai", ai(&s)),
            ("security", security(&s)),
            ("data", data(&s)),
            ("advanced", advanced(&s)),
        ] {
            assert_eq!(rows[0].kind, "header", "{name} does not open with a header");
        }
    }

    /// A toggle or a text row with no key writes into nothing. Headers and
    /// status lines are the only rows allowed to be keyless.
    #[test]
    fn every_writable_row_carries_a_key() {
        let s = S::default();
        for rows in [playback(&s), services(&s), ai(&s), security(&s), data(&s), advanced(&s)] {
            for r in rows {
                let writable = matches!(r.kind.as_str(), "toggle" | "text" | "choice" | "action");
                assert!(!writable || !r.key.is_empty(), "{} has no key", r.label);
            }
        }
    }

    /// A choice row with no stored value must offer its first option rather
    /// than an empty selection the picker cannot show.
    #[test]
    fn a_choice_defaults_to_its_first_option() {
        let row = choice(&S::default(), "ai.model.voice", "Voice", "", &["tiny", "base"]);
        assert_eq!(row.value, "tiny");
        assert_eq!(row.options.len(), 2);
    }

    /// Every "Lock after" choice reads back as itself, and a value that is not
    /// on the list shows as the nearest one.
    #[test]
    fn lock_after_labels_round_trip() {
        for (label, secs) in LOCK_AFTER {
            assert_eq!(lock_after_label(secs), label);
        }
        assert_eq!(lock_after_label(700), "10 minutes");
        assert_eq!(lock_after_label(100_000), "1 hour");
    }

    /// A restore names a folder, so anything but a backup's bare number is
    /// refused before it can become a path.
    #[test]
    fn a_restore_takes_only_a_backup_id() {
        for bad in ["", "..", "../settings", "12/../../x", "abc"] {
            assert!(restore(bad).is_err(), "{bad:?} was accepted");
        }
    }

    /// Home cards default to on: a fresh install shows the whole page, and the
    /// switches are there to take things away.
    #[test]
    fn home_cards_start_on() {
        let s = S::default();
        assert!(HOME_CARDS.iter().all(|(k, _)| s.flag(&format!("{HOME_CARD_PREFIX}{k}"), true)));
    }
}
