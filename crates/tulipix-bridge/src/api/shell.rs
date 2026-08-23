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
    /// "system" | "light" | "dark" | "extra-dark".
    pub theme: String,
    pub reduce_motion: bool,
    pub app_version: String,
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
    let s = load();
    let (badge, overdue) = finances_badge().await;
    Ok(ShellState {
        user: ShellUser {
            display_name: match s.text("profile.name") {
                n if n.trim().is_empty() => "Local user".into(),
                n => n,
            },
            secondary: crate::api::home::library_line().await,
            avatar_emoji: s.text("profile.emoji"),
            mode: "local".into(),
        },
        logo_choice: s.text("profile.logo").parse().unwrap_or(0),
        finances_badge: badge,
        finances_overdue: overdue,
        theme: s.theme.clone(),
        reduce_motion: s.reduce_motion,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    })
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
}
