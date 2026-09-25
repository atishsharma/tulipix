//! Which sections this install shows, in what order, and which one it opens on.
//!
//! Tulipix ships nineteen sections and nobody wants all nineteen. Somebody installs it for
//! Photos and Music and will never open Finances; somebody else wants Finances
//! and Transfer and no media at all. The sidebar was a const list, so there was
//! no way to say so.
//!
//! **Hidden and off are not the same thing**, which is why this is not a
//! checkbox. Hiding Finances should take it out of the sidebar. Turning it
//! *off* should also stop its bill rollover. A single switch has to conflate
//! those, and either leaves work running for something you hid or silently
//! stops work the moment you tidy your sidebar.
//!
//! Lives in core so both front ends read one answer: the Slint sidebar and the
//! Flutter shell must not disagree about what exists.

use crate::settings::Settings;

/// Every section, in the order a fresh install shows them. The ids are a
/// storage format — they are what `section.<id>` is keyed on — so renaming one
/// silently resets that section to shown.
pub const ALL: [&str; 19] = [
    "home", "photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances",
    "feeds", "journal", "kitchen", "papers", "voice", "places", "studio", "archive", "arcade", "settings",
];

/// Bonus sections: shipped, but off until somebody adds one. A fresh install
/// should look like the fourteen it always did, not grow a section per release.
pub const BONUS: [&str; 5] = ["voice", "places", "studio", "archive", "arcade"];

/// Settings is how you get back. It cannot be hidden, and it is pinned last —
/// a way back you have to hunt for is not one.
pub const PINNED: &str = "settings";

/// Always shown: Home is where the sidebar starts and Settings is the way
/// back. Neither is in the panel's list, and neither can be switched.
pub const ALWAYS: [&str; 2] = ["home", PINNED];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// In the sidebar, built at launch, indexing.
    #[default]
    Shown,
    /// Out of the sidebar and not built at launch, but its background work
    /// keeps running — so it is already current the moment you bring it back.
    Hidden,
    /// Out, and its background work stands down. Nothing on disk is touched.
    Off,
}

impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Mode::Shown => "shown",
            Mode::Hidden => "hidden",
            Mode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Mode {
        match s {
            "hidden" => Mode::Hidden,
            "off" => Mode::Off,
            // An unreadable value is a section that disappears, which is worse
            // than one that is shown when it should not be.
            _ => Mode::Shown,
        }
    }

    /// In the sidebar.
    pub fn visible(self) -> bool {
        self == Mode::Shown
    }

    /// Allowed to do background work. Hidden still counts — that is the whole
    /// point of it being a third state.
    pub fn works(self) -> bool {
        self != Mode::Off
    }
}

fn mode_key(id: &str) -> String {
    format!("section.{id}")
}

const ORDER_KEY: &str = "sections.order";
const LANDING_KEY: &str = "shell.landing";

/// One section's mode. Unknown ids answer `Shown`, because a section this build
/// gained and the settings file has not heard of should appear — except a bonus
/// one, which waits to be added.
pub fn mode_of(s: &Settings, id: &str) -> Mode {
    if ALWAYS.contains(&id) {
        return Mode::Shown;
    }
    match s.advanced.get(&mode_key(id)) {
        Some(v) => Mode::parse(v),
        None if BONUS.contains(&id) => Mode::Off,
        None => Mode::Shown,
    }
}

pub fn set_mode(s: &mut Settings, id: &str, mode: Mode) {
    if ALWAYS.contains(&id) {
        return;
    }
    s.advanced.insert(mode_key(id), mode.key().to_string());
}

fn tabs_key(id: &str) -> String {
    format!("section.{id}.tabs-off")
}

/// The tabs switched off inside one section, by the id its page knows them by.
///
/// Stored as what is *off* rather than what is kept, so a build that adds a tab
/// shows it. The opposite — storing the keepers — silently hides every tab
/// added after the day somebody last touched this panel.
pub fn tabs_off(s: &Settings, id: &str) -> Vec<String> {
    s.advanced
        .get(&tabs_key(id))
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Switch one tab on or off. Which tabs a section *has* is the front end's
/// business — this only remembers what it was told, so a tab that no longer
/// exists costs one stale word and nothing else.
pub fn set_tab(s: &mut Settings, id: &str, tab: &str, on: bool) {
    let tab = tab.trim();
    if tab.is_empty() {
        return;
    }
    let mut off = tabs_off(s, id);
    if on {
        off.retain(|t| t != tab);
    } else if !off.iter().any(|t| t == tab) {
        off.push(tab.to_string());
    }
    let key = tabs_key(id);
    if off.is_empty() {
        s.advanced.remove(&key);
    } else {
        s.advanced.insert(key, off.join(","));
    }
}

/// Every switched-off tab as `<section>:<tab>`, which is what the shell hands
/// the front end: one flat list beats ten fields that each mean the same thing.
pub fn tabs_off_all(s: &Settings) -> Vec<String> {
    ALL.iter()
        .flat_map(|id| {
            tabs_off(s, id)
                .into_iter()
                .map(move |t| format!("{id}:{t}"))
        })
        .collect()
}

/// Every section in the saved order, with its mode.
///
/// A saved order that has fallen behind the build — a section added, a section
/// dropped — is repaired here rather than stored: anything missing is appended
/// in `ALL` order and anything unknown is dropped, so the list is always
/// exactly this build's sections.
pub fn ordered(s: &Settings) -> Vec<(&'static str, Mode)> {
    let saved = s.advanced.get(ORDER_KEY).cloned().unwrap_or_default();
    let mut out: Vec<&'static str> = Vec::with_capacity(ALL.len());
    for want in saved.split(',').map(str::trim).filter(|v| !v.is_empty()) {
        if let Some(id) = ALL.iter().find(|a| **a == want).copied() {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    for id in ALL {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    // Home first and Settings last, wherever they were put.
    out.retain(|id| !ALWAYS.contains(id));
    out.insert(0, "home");
    out.push(PINNED);
    out.into_iter().map(|id| (id, mode_of(s, id))).collect()
}

pub fn set_order(s: &mut Settings, ids: &[String]) {
    let clean: Vec<&str> = ids
        .iter()
        .map(|v| v.as_str())
        .filter(|v| ALL.contains(v) && !ALWAYS.contains(v))
        .collect();
    s.advanced.insert(ORDER_KEY.to_string(), clean.join(","));
}

/// The sections drawn in the sidebar, in order.
pub fn visible(s: &Settings) -> Vec<&'static str> {
    ordered(s)
        .into_iter()
        .filter(|(_, m)| m.visible())
        .map(|(id, _)| id)
        .collect()
}

/// Which section opens at launch.
///
/// Falls back rather than opening onto nothing: the saved choice if it is still
/// shown, else the first shown section, else Settings — which is always shown
/// and is the way back.
pub fn landing(s: &Settings) -> &'static str {
    let vis = visible(s);
    let saved = s.advanced.get(LANDING_KEY).map(String::as_str);
    if let Some(id) = saved.and_then(|w| vis.iter().find(|v| **v == w).copied()) {
        return id;
    }
    vis.first().copied().unwrap_or(PINNED)
}

pub fn set_landing(s: &mut Settings, id: &str) {
    if ALL.contains(&id) {
        s.advanced.insert(LANDING_KEY.to_string(), id.to_string());
    }
}

/// A ready-made set: which applications it shows. Home and Settings are always
/// there, so no preset names them.
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    pub ids: &'static [&'static str],
}

const fn p(
    id: &'static str,
    name: &'static str,
    hint: &'static str,
    ids: &'static [&'static str],
) -> Preset {
    Preset { id, name, hint, ids }
}

/// The ready-made sets, in the order the panel shows them. The ids are stored
/// (`sections.preset.last`), so renaming one forgets which preset was picked.
pub const PRESETS: [Preset; 12] = [
    p("all", "Everything", "every application", &[
        "photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances",
        "feeds", "journal", "kitchen", "papers",
    ]),
    p("bonus", "Everything + bonus", "all of them, bonus included", &[
        "photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances",
        "feeds", "journal", "kitchen", "papers", "voice", "places", "studio", "archive",
        "arcade",
    ]),
    p("media", "Media", "watch, listen, read", &["photos", "videos", "music", "books"]),
    p("everyday", "Everyday life", "news, days, meals, money", &[
        "feeds", "journal", "kitchen", "papers", "finances",
    ]),
    p("work", "Work & files", "documents, drives, tools", &[
        "papers", "finances", "cloud", "transfer", "tools", "archive",
    ]),
    p("creator", "Creator", "make things from your library", &[
        "photos", "videos", "music", "studio", "voice",
    ]),
    p("traveller", "Traveller", "trips, photos, spending", &[
        "photos", "places", "journal", "studio", "finances",
    ]),
    p("student", "Student", "reading, notes, papers", &[
        "books", "voice", "papers", "feeds", "tools",
    ]),
    p("family", "Home & family", "photos, food, bills", &[
        "photos", "kitchen", "finances", "papers", "journal",
    ]),
    p("play", "Play", "games, films, music", &["videos", "music", "arcade", "books"]),
    // Nothing that indexes in the background except what you look at.
    p("lean", "Lean", "least in the background", &["photos", "music", "books", "journal"]),
    p("photos", "Just Photos", "one application", &["photos"]),
];

const MINE_KEY: &str = "sections.mine";
const LAST_KEY: &str = "sections.preset.last";
const UNDO_KEY: &str = "sections.undo";
/// Saved presets are named `mine:<name>` wherever a preset id is expected.
pub const MINE: &str = "mine:";

/// Presets somebody saved: (name, the applications it shows), oldest first.
pub fn mine(s: &Settings) -> Vec<(String, Vec<String>)> {
    s.advanced
        .get(MINE_KEY)
        .map(|v| {
            v.split(';')
                .filter_map(|e| e.split_once('='))
                .map(|(n, ids)| {
                    (
                        n.trim().to_string(),
                        ids.split(',')
                            .map(str::trim)
                            .filter(|i| ALL.contains(i))
                            .map(str::to_string)
                            .collect(),
                    )
                })
                .filter(|(n, _)| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn put_mine(s: &mut Settings, list: &[(String, Vec<String>)]) {
    let v: Vec<String> = list
        .iter()
        .map(|(n, ids)| format!("{n}={}", ids.join(",")))
        .collect();
    s.advanced.insert(MINE_KEY.to_string(), v.join(";"));
}

/// Save what is shown now under `name`, replacing one of the same name.
pub fn save_mine(s: &mut Settings, name: &str) -> bool {
    // The separators the key is stored with.
    let name: String = name.chars().filter(|c| !matches!(c, ';' | '=' | ',')).collect();
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let ids: Vec<String> = visible(s)
        .into_iter()
        .filter(|id| !ALWAYS.contains(id))
        .map(str::to_string)
        .collect();
    let mut list = mine(s);
    list.retain(|(n, _)| n != name);
    list.push((name.to_string(), ids));
    put_mine(s, &list);
    s.advanced.insert(LAST_KEY.to_string(), format!("{MINE}{name}"));
    true
}

pub fn delete_mine(s: &mut Settings, name: &str) {
    let mut list = mine(s);
    list.retain(|(n, _)| n != name);
    put_mine(s, &list);
}

/// What a preset id shows. `None` for an unknown one, so a typo is a no-op
/// rather than a wiped sidebar.
fn preset_ids(s: &Settings, id: &str) -> Option<Vec<String>> {
    if let Some(name) = id.strip_prefix(MINE) {
        return mine(s).into_iter().find(|(n, _)| n == name).map(|(_, ids)| ids);
    }
    PRESETS
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.ids.iter().map(|i| i.to_string()).collect())
}

/// Apply a preset: everything in it Shown, everything else Off.
///
/// Off rather than Hidden on purpose — picking "Work & files" is a statement
/// that the media sections should stop working, not just stop showing.
/// Individual sections can be moved to Hidden afterwards. What was there
/// before is kept, once, for [`undo`].
pub fn apply_preset(s: &mut Settings, name: &str) -> bool {
    let Some(on) = preset_ids(s, name) else { return false };
    let before: Vec<String> = ordered(s)
        .into_iter()
        .map(|(id, m)| format!("{id}={}", m.key()))
        .collect();
    s.advanced.insert(UNDO_KEY.to_string(), before.join(","));
    for id in ALL {
        let shown = on.iter().any(|o| o == id);
        set_mode(s, id, if shown { Mode::Shown } else { Mode::Off });
    }
    s.advanced.insert(LAST_KEY.to_string(), name.to_string());
    // The landing may have just been switched off.
    let land = landing(s).to_string();
    set_landing(s, &land);
    true
}

pub fn can_undo(s: &Settings) -> bool {
    s.advanced.contains_key(UNDO_KEY)
}

/// Put back what the last preset replaced. One level: an undo you can undo is
/// a history panel, and this is a button.
pub fn undo(s: &mut Settings) -> bool {
    let Some(v) = s.advanced.remove(UNDO_KEY) else { return false };
    for (id, m) in v.split(',').filter_map(|e| e.split_once('=')) {
        if ALL.contains(&id) {
            set_mode(s, id, Mode::parse(m));
        }
    }
    s.advanced.remove(LAST_KEY);
    let land = landing(s).to_string();
    set_landing(s, &land);
    true
}

/// The preset picked last, even if it has been changed since — the strip says
/// "changed" rather than forgetting it. Empty if none.
pub fn last_preset(s: &Settings) -> String {
    s.advanced.get(LAST_KEY).cloned().unwrap_or_default()
}

/// Which preset the current shape matches, or "custom".
pub fn current_preset(s: &Settings) -> String {
    let vis: Vec<&str> = visible(s)
        .into_iter()
        .filter(|id| !ALWAYS.contains(id))
        .collect();
    let same = |ids: &[String]| ids.len() == vis.len() && ids.iter().all(|i| vis.contains(&i.as_str()));
    // A saved one first: somebody who saved "My laptop" wants to see that name,
    // not the built-in it happens to equal.
    let last = last_preset(s);
    if let Some(ids) = preset_ids(s, &last)
        && same(ids.as_slice())
    {
        return last;
    }
    for p in &PRESETS {
        let ids: Vec<String> = p.ids.iter().map(|i| i.to_string()).collect();
        if same(ids.as_slice()) {
            return p.id.to_string();
        }
    }
    for (n, ids) in mine(s) {
        if same(ids.as_slice()) {
            return format!("{MINE}{n}");
        }
    }
    "custom".to_string()
}

// ── groups ──────────────────────────────────────────────────────────────────

const GROUPS_KEY: &str = "sections.groups";
/// What a fresh install is grouped as, on the default order.
const DEFAULT_GROUPS: &str = "Media=photos;Files & devices=cloud;Everyday=finances;Bonus=voice";

/// The sidebar's groups, in order: (name, the section it starts at). A group
/// starts at a section rather than holding a list, so reordering needs no
/// second bookkeeping — a section belongs to the group above it. An empty
/// anchor is a group at the very end with nothing in it yet.
pub fn groups(s: &Settings) -> Vec<(String, String)> {
    let raw = s
        .advanced
        .get(GROUPS_KEY)
        .map(String::as_str)
        .unwrap_or(DEFAULT_GROUPS);
    raw.split(';')
        .filter_map(|e| e.split_once('='))
        .map(|(n, a)| (n.trim().to_string(), a.trim().to_string()))
        .filter(|(n, a)| !n.is_empty() && (a.is_empty() || ALL.contains(&a.as_str())))
        .collect()
}

/// `groups` is `name=anchor` per group. Names lose the separators.
pub fn set_groups(s: &mut Settings, groups: &[String]) {
    let clean: Vec<String> = groups
        .iter()
        .filter_map(|g| g.split_once('='))
        .map(|(n, a)| {
            let n: String = n.chars().filter(|c| !matches!(c, ';' | '=')).collect();
            (n.trim().to_string(), a.trim().to_string())
        })
        .filter(|(n, a)| !n.is_empty() && (a.is_empty() || ALL.contains(&a.as_str())))
        .map(|(n, a)| format!("{n}={a}"))
        .collect();
    s.advanced.insert(GROUPS_KEY.to_string(), clean.join(";"));
}

/// Back to the default order and the default groups.
pub fn reset_layout(s: &mut Settings) {
    s.advanced.remove(ORDER_KEY);
    s.advanced.remove(GROUPS_KEY);
}

/// The dividers the sidebar draws: (the first *shown* section of a group, its
/// name). A group with nothing shown draws nothing; a divider for an empty
/// run is a line with nothing under it.
pub fn sidebar_groups(s: &Settings) -> Vec<(&'static str, String)> {
    let gs = groups(s);
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for (id, mode) in ordered(s) {
        if ALWAYS.contains(&id) {
            continue;
        }
        if let Some((n, _)) = gs.iter().rev().find(|(_, a)| a == id) {
            pending = Some(n.clone());
        }
        if mode.visible()
            && let Some(n) = pending.take()
        {
            out.push((id, n));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Settings {
        Settings::default()
    }

    #[test]
    fn a_fresh_install_shows_everything_but_the_bonus_sections() {
        let s = blank();
        assert_eq!(visible(&s).len(), ALL.len() - BONUS.len());
        assert_eq!(mode_of(&s, "voice"), Mode::Off);
        assert_eq!(current_preset(&s), "all");
        assert_eq!(landing(&s), "home");
    }

    #[test]
    fn settings_cannot_be_hidden_and_stays_last() {
        let mut s = blank();
        set_mode(&mut s, PINNED, Mode::Off);
        assert_eq!(mode_of(&s, PINNED), Mode::Shown);
        set_order(&mut s, &["settings".into(), "photos".into(), "home".into()]);
        let ids: Vec<&str> = ordered(&s).into_iter().map(|(id, _)| id).collect();
        assert_eq!(*ids.last().unwrap(), PINNED);
        // And it never makes it into the stored order in the first place.
        assert!(!s.advanced[ORDER_KEY].contains("settings"));
    }

    #[test]
    fn hidden_still_works_and_off_does_not() {
        let mut s = blank();
        set_mode(&mut s, "finances", Mode::Hidden);
        assert!(!mode_of(&s, "finances").visible());
        assert!(mode_of(&s, "finances").works(), "hidden must keep indexing");
        set_mode(&mut s, "finances", Mode::Off);
        assert!(!mode_of(&s, "finances").works());
    }

    #[test]
    fn a_saved_order_missing_a_section_is_repaired_not_obeyed() {
        // A settings file written by a build with fewer sections must not make
        // the new one vanish.
        let mut s = blank();
        s.advanced
            .insert(ORDER_KEY.into(), "music,photos,nonesuch".into());
        let ids: Vec<&str> = ordered(&s).into_iter().map(|(id, _)| id).collect();
        assert_eq!(&ids[..3], &["home", "music", "photos"]);
        assert_eq!(ids.len(), ALL.len());
        assert!(!ids.contains(&"nonesuch"));
        for id in ALL {
            assert!(ids.contains(&id), "{id} went missing");
        }
    }

    #[test]
    fn landing_falls_back_when_its_section_goes() {
        let mut s = blank();
        set_landing(&mut s, "music");
        assert_eq!(landing(&s), "music");
        set_mode(&mut s, "music", Mode::Hidden);
        assert_ne!(landing(&s), "music");
        assert!(visible(&s).contains(&landing(&s)));
    }

    #[test]
    fn everything_off_still_opens_on_settings() {
        let mut s = blank();
        for id in ALL {
            set_mode(&mut s, id, Mode::Off);
        }
        // Home and Settings are not switches.
        assert_eq!(visible(&s), vec!["home", PINNED]);
        assert_eq!(landing(&s), "home");
    }

    #[test]
    fn a_tab_switched_off_is_remembered_by_section() {
        let mut s = blank();
        assert!(tabs_off(&s, "photos").is_empty());
        set_tab(&mut s, "photos", "places", false);
        set_tab(&mut s, "photos", "dedupe", false);
        // Twice off is still once off.
        set_tab(&mut s, "photos", "places", false);
        assert_eq!(tabs_off(&s, "photos"), vec!["places", "dedupe"]);
        // Sections do not share a list.
        assert!(tabs_off(&s, "music").is_empty());

        assert_eq!(tabs_off_all(&s), vec!["photos:places", "photos:dedupe"]);

        set_tab(&mut s, "photos", "places", true);
        set_tab(&mut s, "photos", "dedupe", true);
        assert!(tabs_off(&s, "photos").is_empty());
        // And an empty list leaves no key behind to be parsed back as [""].
        assert!(!s.advanced.contains_key(&tabs_key("photos")));
    }

    #[test]
    fn a_bonus_section_once_added_stays() {
        let mut s = blank();
        set_mode(&mut s, "voice", Mode::Shown);
        assert!(visible(&s).contains(&"voice"));
        // Everything is the core set; a bonus section is not part of it.
        assert_eq!(current_preset(&s), "custom");
        assert!(apply_preset(&mut s, "all"));
        assert_eq!(mode_of(&s, "voice"), Mode::Off);
    }

    #[test]
    fn presets_round_trip() {
        let mut s = blank();
        assert!(apply_preset(&mut s, "work"));
        assert_eq!(current_preset(&s), "work");
        assert!(!visible(&s).contains(&"music"));
        assert!(visible(&s).contains(&landing(&s)));
        assert!(!apply_preset(&mut s, "nonesuch"));
    }

    #[test]
    fn a_preset_can_be_undone_once() {
        let mut s = blank();
        set_mode(&mut s, "music", Mode::Hidden);
        assert!(apply_preset(&mut s, "photos"));
        assert_eq!(current_preset(&s), "photos");
        assert_eq!(mode_of(&s, "music"), Mode::Off);
        assert!(undo(&mut s));
        assert_eq!(mode_of(&s, "music"), Mode::Hidden);
        assert_eq!(mode_of(&s, "voice"), Mode::Off);
        assert!(!can_undo(&s));
        assert!(!undo(&mut s));
    }

    #[test]
    fn a_changed_preset_is_still_remembered() {
        let mut s = blank();
        apply_preset(&mut s, "media");
        set_mode(&mut s, "voice", Mode::Shown);
        assert_eq!(current_preset(&s), "custom");
        assert_eq!(last_preset(&s), "media");
    }

    #[test]
    fn saved_presets_round_trip() {
        let mut s = blank();
        apply_preset(&mut s, "media");
        set_mode(&mut s, "arcade", Mode::Shown);
        assert!(save_mine(&mut s, "My; laptop"));
        assert!(!save_mine(&mut s, "  "));
        assert_eq!(current_preset(&s), "mine:My laptop");
        apply_preset(&mut s, "all");
        assert!(apply_preset(&mut s, "mine:My laptop"));
        assert_eq!(mode_of(&s, "arcade"), Mode::Shown);
        delete_mine(&mut s, "My laptop");
        assert!(mine(&s).is_empty());
        assert!(!apply_preset(&mut s, "mine:My laptop"));
    }

    #[test]
    fn every_preset_names_real_sections() {
        for p in &PRESETS {
            for id in p.ids {
                assert!(ALL.contains(id) && !ALWAYS.contains(id), "{} names {id}", p.id);
            }
        }
    }

    #[test]
    fn a_group_divider_moves_to_its_first_shown_section() {
        let mut s = blank();
        let g = sidebar_groups(&s);
        assert_eq!(g[0], ("photos", "Media".to_string()));
        // The bonus group has nothing shown, so it draws nothing.
        assert!(!g.iter().any(|(_, n)| n == "Bonus"));
        set_mode(&mut s, "photos", Mode::Off);
        assert_eq!(sidebar_groups(&s)[0], ("videos", "Media".to_string()));
        set_groups(&mut s, &["Mine=music".into(), "Bad=nonesuch".into(), "End=".into()]);
        assert_eq!(groups(&s).len(), 2);
        assert_eq!(sidebar_groups(&s), vec![("music", "Mine".to_string())]);
        reset_layout(&mut s);
        assert_eq!(groups(&s)[0].0, "Media");
    }
}
