//! The shell: the sidebar, the profile behind it, and the two live figures it
//! carries -- Finances' due-soon badge and the app health lamp.
//!
//! Small on purpose. Everything here is either read straight out of
//! `Settings.advanced` or counted with one query, and none of it belongs to a
//! section: the sidebar is drawn beside every page, so it cannot be owned by
//! one of them.

use anyhow::Result;

/// Who the app thinks you are, and how big your library is.
pub struct ShellUser {
    pub display_name: String,
    /// "12,304 items · 41 GB" -- the same line the Home header prints, from the
    /// same count.
    pub secondary: String,
    pub avatar_emoji: String,
    /// The cropped profile photo, or empty -- the emoji stands in.
    pub avatar_path: String,
    /// Everything is on this machine. There is no account tier yet, and the
    /// pill says so rather than implying one is coming.
    pub mode: String,
}

pub struct ShellState {
    pub user: ShellUser,
    /// 0 default · 1 colour · 2 dark · 3 white · 4 India.
    pub logo_choice: i32,
    /// Bills and subscriptions coming due inside the alert lead.
    pub finances_badge: i32,
    /// Whether that count contains something already late. An overdue bill and
    /// a bill due on Friday are not the same news, and one dot colour for both
    /// teaches you to ignore the colour.
    pub finances_overdue: bool,
    /// Unread articles in Feeds; 0 until Feeds has been opened.
    pub feeds_unread: i32,
    /// "system" | "light" | "dark" | "extra-dark".
    pub theme: String,
    pub reduce_motion: bool,
    pub app_version: String,
    /// "bar" | "square" | "pill" -- which desktop widget the caption row's
    /// button opens. Settings writes `ui.mini-widget.style` and the Slint build
    /// reads it in `miniwin::wire`; nothing on the Flutter side read it back,
    /// so the picker's choice lasted exactly as long as the session that made
    /// it. Empty means "never chosen", and the default stands.
    pub mini_widget_style: String,
    /// `ui.design-language`: which material the Music section is drawn in.
    /// The Slint build writes `standard` | `clay` | `skeuo` to the same key and
    /// the Flutter build adds `neumorphism` | `glass` | `expressive`; each build
    /// treats a name it does not know as Standard. Empty means never chosen.
    pub design_language: String,
    /// "Follow system accent": the desktop's accent as `#rrggbb`, or empty
    /// when the switch is off or the desktop reports none. Every section
    /// accent takes it.
    pub system_accent: String,
    /// "Honour OS font scale". Off pins text at 100 % whatever the OS asks.
    pub follow_os_font_scale: bool,
    /// The sections in the sidebar, in order, as Settings → Sections left them.
    /// Ids from `tulipix_core::sections::ALL`. The shell builds these pages and
    /// no others — an IndexedStack that builds all ten at launch is ten pages
    /// of state for however many you actually opened.
    pub sections: Vec<String>,
    /// Which one opens at launch, already resolved against what is shown.
    pub landing: String,
    /// Tabs switched off inside sections, as `<section>:<tab>`. Flat rather
    /// than a map per section: every page reads the same one line of it, and a
    /// map would be ten fields that all mean the same thing.
    pub tabs_off: Vec<String>,
    /// The sidebar's group dividers as `<section>:<name>`, each on the first
    /// shown section of its group. Empty when dividers are switched off.
    pub sidebar_groups: Vec<String>,
    /// When the sidebar does not fit: "shrink" | "scroll" | "more".
    pub sidebar_overflow: String,
    /// Material 3 Expressive's colours: where the seed comes from, "cover" |
    /// "desktop" | "pick". Cover by default.
    pub bloom_source: String,
    /// The picked seed as `#rrggbb`; empty until one is picked.
    pub bloom_seed: String,
    /// The desktop's accent as `#rrggbb`, read only while it is the seed:
    /// on Linux each read runs `gsettings`.
    pub bloom_desktop: String,
    /// A Flutter `DynamicSchemeVariant` name; `tonalSpot` by default.
    pub bloom_style: String,
    /// 0 standard, 0.5 medium, 1 high.
    pub bloom_contrast: f64,
    /// Section colours shifted toward the seed.
    pub bloom_harmonise: bool,
}

pub enum ShellCmd {
    Refresh,
    /// Name, avatar emoji, sidebar logo. Empty name or emoji clears the
    /// override rather than storing a blank.
    SaveProfile { name: String, emoji: String, logo: i32 },
    SetTheme { theme: String },
}

/// How many days ahead the badge counts. The same lead the Finances section's
/// own alert list uses -- two answers to "what is due soon" would teach you to
/// trust neither.
const BADGE_LEAD_DAYS: i64 = 7;

/// Longer than this and the sidebar's name row has nowhere to put it.
const NAME_MAX: usize = 32;

pub async fn shell_dispatch(cmd: ShellCmd) -> Result<ShellState> {
    match cmd {
        ShellCmd::Refresh => {}
        ShellCmd::SaveProfile { name, emoji, logo } => {
            let name = clamp_name(&name);
            put("profile.name", &name);
            put("profile.emoji", emoji.trim());
            put("profile.logo", &if logo == 0 { String::new() } else { logo.to_string() });
        }
        ShellCmd::SetTheme { theme } => {
            let mut s = load();
            s.theme = theme;
            save(s);
        }
    }
    snapshot().await
}

async fn snapshot() -> Result<ShellState> {
    // The first snapshot is taken at launch; the auto-rescan loop and the live
    // folder watcher start with it.
    crate::api::maintenance::start_auto_rescan();
    crate::api::maintenance::start_fs_watcher();
    // The photo indexer's own loop. It lived in tulipix-app's runtime, so on
    // this build nothing ever picked the queue up without a button press.
    crate::api::photos::start_ai_indexer();
    // What Papers, Feeds and Cloud do with their pages closed: the watched
    // folder and reminders, the half-hourly fetch, scheduled syncs. Each keeps
    // its own pace, so the extra snapshots a Save sends cost nothing.
    tokio::spawn(crate::api::papers::background_tick());
    tokio::spawn(crate::api::feeds::background_tick());
    tokio::spawn(crate::api::cloud::run_due_jobs());
    let s = load();
    let (badge, overdue) = finances_badge().await;
    remind_bills(&s, badge, overdue);
    Ok(ShellState {
        user: ShellUser {
            display_name: match s.text("profile.name") {
                n if n.trim().is_empty() => "Local user".into(),
                n => n,
            },
            secondary: crate::api::home::library_line().await,
            avatar_emoji: s.text("profile.emoji"),
            avatar_path: picture("avatar.png"),
            mode: "local".into(),
        },
        logo_choice: s.text("profile.logo").parse().unwrap_or(0),
        finances_badge: badge,
        finances_overdue: overdue,
        feeds_unread: crate::api::feeds::unread_count().await,
        theme: s.theme.clone(),
        reduce_motion: s.reduce_motion,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        mini_widget_style: s.text("ui.mini-widget.style"),
        design_language: s.text("ui.design-language"),
        system_accent: if s.flag("follow-system-accent", false) {
            tulipix_platform::accent::read_system_accent()
                .map(|c| c.to_css())
                .unwrap_or_default()
        } else {
            String::new()
        },
        follow_os_font_scale: s.flag("follow-os-font-scale", true),
        sections: tulipix_core::sections::visible(&s)
            .into_iter()
            .map(str::to_string)
            .collect(),
        landing: tulipix_core::sections::landing(&s).into(),
        tabs_off: tulipix_core::sections::tabs_off_all(&s),
        sidebar_groups: if s.flag("sidebar.dividers", true) {
            tulipix_core::sections::sidebar_groups(&s)
                .into_iter()
                .map(|(id, n)| format!("{id}:{n}"))
                .collect()
        } else {
            Vec::new()
        },
        sidebar_overflow: crate::api::settings::sidebar_overflow(&s),
        bloom_source: match s.text("ui.bloom.source").as_str() {
            v @ ("desktop" | "pick") => v.into(),
            _ => "cover".into(),
        },
        bloom_seed: s.text("ui.bloom.seed"),
        bloom_desktop: if s.text("ui.design-language") == "expressive"
            && s.text("ui.bloom.source") == "desktop"
        {
            desktop_accent()
        } else {
            String::new()
        },
        bloom_style: match s.text("ui.bloom.style") {
            v if v.is_empty() => "tonalSpot".into(),
            v => v,
        },
        bloom_contrast: s.text("ui.bloom.contrast").parse().unwrap_or(0.0),
        bloom_harmonise: s.flag("ui.bloom.harmonise", false),
    })
}

/// The desktop's accent as `#rrggbb`, or empty when it reports none. For the
/// Bloom colours popup, which shows it before it is the seed.
pub fn desktop_accent() -> String {
    tulipix_platform::accent::read_system_accent()
        .map(|c| c.to_css())
        .unwrap_or_default()
}

/// Once a day at most, a notification when bills are coming due, so the badge
/// is not the only thing that says so.
fn remind_bills(s: &tulipix_core::settings::Settings, badge: i32, overdue: bool) {
    if badge == 0 {
        return;
    }
    let today = chrono::Local::now().date_naive().to_string();
    if s.text("notify.bills-day") == today {
        return;
    }
    put("notify.bills-day", &today);
    let body = if overdue {
        format!("{badge} due within {BADGE_LEAD_DAYS} days, and one is already late.")
    } else {
        format!("{badge} due within {BADGE_LEAD_DAYS} days.")
    };
    let title = if badge == 1 { "A bill is due soon" } else { "Bills are due soon" };
    crate::api::maintenance::notify(title, &body);
}

/// Obligations due inside the lead, and whether any of them is already late.
///
/// Two counts rather than one query returning a flag: the badge shows the
/// first number and tints itself with the second, and a row that is both due
/// soon and overdue must not be counted twice.
async fn finances_badge() -> (i32, bool) {
    let Ok(pool) = crate::db::finances_pool().await else { return (0, false) };
    let today = chrono::Local::now().date_naive();
    let count = tulipix_finances::obligations::badge_count(pool, today, BADGE_LEAD_DAYS)
        .await
        .unwrap_or(0);
    if count == 0 {
        return (0, false);
    }
    let overdue: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM obligations WHERE status = 'overdue'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    (count as i32, overdue > 0)
}

fn clamp_name(name: &str) -> String {
    name.trim().chars().take(NAME_MAX).collect()
}

/// `<data>/profile/{avatar,cover}.png` -- the fixed names the Slint build reads
/// and writes too, so both builds show the same pictures.
fn picture_file(name: &str) -> Option<std::path::PathBuf> {
    tulipix_core::paths::data_dir().map(|d| d.join("profile").join(name))
}

/// The picture's path if it is on disk, else empty.
pub(crate) fn picture(name: &str) -> String {
    picture_file(name)
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Replace a profile picture with a PNG the page already cropped, or remove it
/// when `png` is empty.
pub(crate) fn store_picture(name: &str, png: &[u8]) -> Result<()> {
    let path = picture_file(name).ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?;
    write_picture(&path, png)
}

/// Written beside and renamed over, so a crash mid-write never leaves half a
/// file that both builds would then fail to decode.
fn write_picture(path: &std::path::Path, png: &[u8]) -> Result<()> {
    if png.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    anyhow::ensure!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "the cropped picture is not a PNG");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("png.part");
    std::fs::write(&part, png)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

pub(crate) fn load() -> tulipix_core::settings::Settings {
    tulipix_core::settings::Settings::load().unwrap_or_default()
}

pub(crate) fn save(s: tulipix_core::settings::Settings) {
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, "shell: could not save settings");
    }
}

/// Write one `advanced` key. An empty value removes it rather than storing a
/// blank, so "unset" and "set to nothing" stay the same thing.
pub(crate) fn put(key: &str, value: &str) {
    let mut s = load();
    if value.trim().is_empty() {
        s.advanced.remove(key);
    } else {
        s.advanced.insert(key.to_string(), value.trim().to_string());
    }
    save(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name long enough to push the mode pill off the row is cut, not
    /// ellipsised in Dart -- the stored value is what the profile page shows
    /// back, and it should be what was actually kept.
    #[test]
    fn a_profile_name_is_trimmed_and_capped() {
        assert_eq!(clamp_name("  Ada  "), "Ada");
        assert_eq!(clamp_name(&"x".repeat(80)).len(), NAME_MAX);
        assert_eq!(clamp_name("   "), "");
    }

    /// A picture is written, replaced, refused when it is not a PNG, and
    /// removed -- and removing one that is already gone is not an error.
    #[test]
    fn a_profile_picture_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile").join("avatar.png");
        let png = b"\x89PNG\r\n\x1a\nfirst".to_vec();
        write_picture(&path, &png).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), png);
        let next = b"\x89PNG\r\n\x1a\nsecond".to_vec();
        write_picture(&path, &next).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), next);
        assert!(write_picture(&path, b"GIF89a").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), next);
        write_picture(&path, &[]).unwrap();
        assert!(!path.exists());
        write_picture(&path, &[]).unwrap();
    }
}
