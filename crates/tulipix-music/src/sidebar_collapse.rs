//! `np.p4.music.sidebar-collapse` — music-section secondary sidebar.
//!
//! Panes: Library / Playlists / Podcasts / Audiobooks / Radio / YouTube /
//! Folders. Collapsible with Cmd/Ctrl+B; the expanded width is remembered so a
//! toggle restores it. Pure state the UI binds to.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane { Library, Playlists, Podcasts, Audiobooks, Radio, YouTube, Folders }

impl Pane {
    pub const ALL: [Pane; 7] = [Pane::Library, Pane::Playlists, Pane::Podcasts, Pane::Audiobooks, Pane::Radio, Pane::YouTube, Pane::Folders];
    pub fn label(self) -> &'static str {
        match self {
            Pane::Library => "Library", Pane::Playlists => "Playlists", Pane::Podcasts => "Podcasts",
            Pane::Audiobooks => "Audiobooks", Pane::Radio => "Radio", Pane::YouTube => "YouTube", Pane::Folders => "Folders",
        }
    }
}

pub const MIN_WIDTH: f64 = 180.0;
pub const MAX_WIDTH: f64 = 420.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sidebar {
    pub active: Pane,
    pub collapsed: bool,
    /// Remembered expanded width, restored on toggle.
    pub width: f64,
}

impl Default for Sidebar {
    fn default() -> Self { Self { active: Pane::Library, collapsed: false, width: 240.0 } }
}

impl Sidebar {
    /// Cmd/Ctrl+B handler.
    pub fn toggle(&mut self) { self.collapsed = !self.collapsed; }

    pub fn set_width(&mut self, w: f64) { self.width = w.clamp(MIN_WIDTH, MAX_WIDTH); }

    /// Effective rendered width (0 when collapsed).
    pub fn rendered_width(&self) -> f64 { if self.collapsed { 0.0 } else { self.width } }

    pub fn select(&mut self, p: Pane) { self.active = p; if self.collapsed { self.collapsed = false; } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_preserves_width() {
        let mut s = Sidebar::default();
        s.set_width(300.0);
        s.toggle();
        assert_eq!(s.rendered_width(), 0.0);
        s.toggle();
        assert_eq!(s.rendered_width(), 300.0); // restored
    }

    #[test]
    fn width_clamps_and_select_expands() {
        let mut s = Sidebar::default();
        s.set_width(9999.0);
        assert_eq!(s.width, MAX_WIDTH);
        s.collapsed = true;
        s.select(Pane::Radio);
        assert!(!s.collapsed);
        assert_eq!(s.active, Pane::Radio);
        assert_eq!(Pane::ALL.len(), 7);
    }
}
