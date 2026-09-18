//! Which sections this install shows, in what order, and which one it opens on.
//!
//! Tulipix ships ten sections and nobody wants all ten. Somebody installs it for
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
pub const ALL: [&str; 10] = [
    "home", "photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances",
    "settings",
];

/// Settings is how you get back. It cannot be hidden, and it is pinned last —
/// a way back you have to hunt for is not one.
pub const PINNED: &str = "settings";

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
/// gained and the settings file has not heard of should appear.
pub fn mode_of(s: &Settings, id: &str) -> Mode {
    if id == PINNED {
        return Mode::Shown;
    }
    s.advanced
        .get(&mode_key(id))
        .map(|v| Mode::parse(v))
        .unwrap_or_default()
}

pub fn set_mode(s: &mut Settings, id: &str, mode: Mode) {
    if id == PINNED {
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
    // Settings goes last wherever it was put.
    out.retain(|id| *id != PINNED);
    out.push(PINNED);
    out.into_iter().map(|id| (id, mode_of(s, id))).collect()
}

pub fn set_order(s: &mut Settings, ids: &[String]) {
    let clean: Vec<&str> = ids
        .iter()
        .map(|v| v.as_str())
        .filter(|v| ALL.contains(v) && *v != PINNED)
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

/// The ready-made shapes. `None` for an unknown name, so a typo is a no-op
/// rather than a wiped sidebar.
pub fn preset(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "all" => &ALL,
        "media" => &["home", "photos", "videos", "music", "books", "settings"],
        "work" => &["home", "cloud", "tools", "transfer", "finances", "settings"],
        "photos" => &["photos", "settings"],
        // Nothing that indexes in the background except what you look at.
        "lean" => &["home", "photos", "music", "settings"],
        _ => return None,
    })
}

/// Apply a preset: everything in it Shown, everything else Off.
///
/// Off rather than Hidden on purpose — picking "Work only" is a statement that
/// the media sections should stop working, not just stop showing. Individual
/// sections can be moved to Hidden afterwards.
pub fn apply_preset(s: &mut Settings, name: &str) -> bool {
    let Some(on) = preset(name) else { return false };
    for id in ALL {
        set_mode(
            s,
            id,
            if on.contains(&id) { Mode::Shown } else { Mode::Off },
        );
    }
    // The landing may have just been switched off.
    let land = landing(s).to_string();
    set_landing(s, &land);
    true
}

/// Which preset the current shape matches, or "custom".
pub fn current_preset(s: &Settings) -> &'static str {
    let vis = visible(s);
    for name in ["all", "media", "work", "photos", "lean"] {
        let Some(on) = preset(name) else { continue };
        if vis.len() == on.len() && on.iter().all(|id| vis.contains(id)) {
            return name;
        }
    }
    "custom"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Settings {
        Settings::default()
    }

    #[test]
    fn a_fresh_install_shows_everything() {
        let s = blank();
        assert_eq!(visible(&s).len(), ALL.len());
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
        assert_eq!(&ids[..2], &["music", "photos"]);
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
        assert_eq!(visible(&s), vec![PINNED]);
        assert_eq!(landing(&s), PINNED);
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
    fn presets_round_trip() {
        let mut s = blank();
        assert!(apply_preset(&mut s, "work"));
        assert_eq!(current_preset(&s), "work");
        assert!(!visible(&s).contains(&"music"));
        assert!(visible(&s).contains(&landing(&s)));
        assert!(!apply_preset(&mut s, "nonesuch"));
    }
}
