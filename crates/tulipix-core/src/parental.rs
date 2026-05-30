//! Parental controls — applies on top of a [`profiles_managed::ManagedProfile`]
//! to gate playback by content rating, section allow-list, and time-of-day.
//!
//! The decision lives outside the player so the same call resolves
//! "can this profile see / list / play / interact with X" from the UI, the
//! MCP server, and the watch-party host.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rating {
    G,
    PG,
    Pg13,
    R,
    Nc17,
    Tv7,
    TvY,
    TvY7,
    TvG,
    TvPg,
    Tv14,
    TvMa,
    NotRated,
}

impl Rating {
    /// Comparable severity score — higher = more restricted. The "rating
    /// ceiling" check just picks the higher score and compares.
    pub fn severity(self) -> u8 {
        match self {
            Rating::G | Rating::TvY | Rating::TvG => 1,
            Rating::PG | Rating::TvY7 | Rating::Tv7 => 2,
            Rating::Pg13 | Rating::TvPg => 3,
            Rating::Tv14 => 4,
            Rating::R | Rating::TvMa => 5,
            Rating::Nc17 => 6,
            Rating::NotRated => 7, // hide by default when a ceiling is set
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_uppercase().replace('-', "").as_str() {
            "G" => Rating::G,
            "PG" => Rating::PG,
            "PG13" => Rating::Pg13,
            "R" => Rating::R,
            "NC17" => Rating::Nc17,
            "TVY" => Rating::TvY,
            "TVY7" => Rating::TvY7,
            "TV7" => Rating::Tv7,
            "TVG" => Rating::TvG,
            "TVPG" => Rating::TvPg,
            "TV14" => Rating::Tv14,
            "TVMA" => Rating::TvMa,
            "NR" | "UNRATED" | "" => Rating::NotRated,
            _ => return None,
        })
    }
}

/// Time-of-day allow window per weekday. `start_minute <= now < end_minute`
/// (minutes since local midnight). When `start == end` the window is closed
/// (zero-length), letting us model "completely blocked weekdays".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start_minute: i32,
    pub end_minute: i32,
}

impl TimeWindow {
    pub fn always() -> Self {
        Self {
            start_minute: 0,
            end_minute: 24 * 60,
        }
    }
    pub fn closed() -> Self {
        Self::default()
    }
    pub fn contains(&self, minute: i32) -> bool {
        // Allow windows that wrap midnight (start > end) for night-owl tweens.
        if self.start_minute <= self.end_minute {
            minute >= self.start_minute && minute < self.end_minute
        } else {
            minute >= self.start_minute || minute < self.end_minute
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentalPolicy {
    pub max_rating: Option<Rating>,
    pub allowed_sections: BTreeSet<String>,
    /// Sunday=0, Saturday=6. `None` for a day means use `default_window`.
    pub per_day_window: [Option<TimeWindow>; 7],
    pub default_window: TimeWindow,
}

impl Default for ParentalPolicy {
    fn default() -> Self {
        let mut sections = BTreeSet::new();
        for s in ["photos", "videos", "music", "books"] {
            sections.insert(s.into());
        }
        Self {
            max_rating: None,
            allowed_sections: sections,
            per_day_window: Default::default(),
            default_window: TimeWindow::always(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackContext {
    pub section: String,
    pub item_rating: Rating,
    pub weekday: u8, // 0..=6 (Sun..Sat)
    pub minute_of_day: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    BlockedRating(Rating),
    BlockedSection,
    BlockedTimeOfDay,
}

impl ParentalPolicy {
    pub fn check(&self, ctx: &PlaybackContext) -> Decision {
        if !self.allowed_sections.contains(&ctx.section) {
            return Decision::BlockedSection;
        }
        let day = ctx.weekday.min(6) as usize;
        let window = self.per_day_window[day]
            .as_ref()
            .unwrap_or(&self.default_window);
        if !window.contains(ctx.minute_of_day) {
            return Decision::BlockedTimeOfDay;
        }
        if let Some(cap) = self.max_rating {
            if ctx.item_rating.severity() > cap.severity() {
                return Decision::BlockedRating(ctx.item_rating);
            }
        }
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(section: &str, rating: Rating, day: u8, minute: i32) -> PlaybackContext {
        PlaybackContext {
            section: section.into(),
            item_rating: rating,
            weekday: day,
            minute_of_day: minute,
        }
    }

    #[test]
    fn rating_parsing_round_trip() {
        assert_eq!(Rating::parse("PG-13"), Some(Rating::Pg13));
        assert_eq!(Rating::parse("tvma"), Some(Rating::TvMa));
        assert_eq!(Rating::parse("NR"), Some(Rating::NotRated));
        assert_eq!(Rating::parse(""), Some(Rating::NotRated));
        assert!(Rating::parse("WAT").is_none());
    }

    #[test]
    fn rating_severity_orders_expectedly() {
        assert!(Rating::G.severity() < Rating::Pg13.severity());
        assert!(Rating::Pg13.severity() < Rating::R.severity());
        assert!(Rating::R.severity() < Rating::Nc17.severity());
        assert!(Rating::NotRated.severity() > Rating::Nc17.severity());
    }

    #[test]
    fn allow_when_default_policy_and_known_section() {
        let p = ParentalPolicy::default();
        assert_eq!(p.check(&ctx("videos", Rating::G, 3, 600)), Decision::Allow);
    }

    #[test]
    fn block_unknown_section() {
        let p = ParentalPolicy::default();
        assert_eq!(
            p.check(&ctx("tools", Rating::G, 3, 600)),
            Decision::BlockedSection
        );
    }

    #[test]
    fn rating_ceiling_blocks_higher() {
        let mut p = ParentalPolicy::default();
        p.max_rating = Some(Rating::Pg13);
        assert_eq!(p.check(&ctx("videos", Rating::G, 3, 600)), Decision::Allow);
        assert_eq!(p.check(&ctx("videos", Rating::Pg13, 3, 600)), Decision::Allow);
        match p.check(&ctx("videos", Rating::R, 3, 600)) {
            Decision::BlockedRating(Rating::R) => {}
            other => panic!("expected R block, got {other:?}"),
        }
    }

    #[test]
    fn time_window_blocks_outside() {
        let mut p = ParentalPolicy::default();
        p.default_window = TimeWindow {
            start_minute: 8 * 60,
            end_minute: 20 * 60,
        };
        assert_eq!(p.check(&ctx("videos", Rating::G, 1, 7 * 60)), Decision::BlockedTimeOfDay);
        assert_eq!(p.check(&ctx("videos", Rating::G, 1, 12 * 60)), Decision::Allow);
        assert_eq!(
            p.check(&ctx("videos", Rating::G, 1, 21 * 60)),
            Decision::BlockedTimeOfDay
        );
    }

    #[test]
    fn time_window_wraps_midnight() {
        let mut p = ParentalPolicy::default();
        p.default_window = TimeWindow {
            start_minute: 20 * 60,
            end_minute: 2 * 60,
        };
        assert_eq!(p.check(&ctx("videos", Rating::G, 1, 21 * 60)), Decision::Allow);
        assert_eq!(p.check(&ctx("videos", Rating::G, 1, 60)), Decision::Allow);
        assert_eq!(
            p.check(&ctx("videos", Rating::G, 1, 12 * 60)),
            Decision::BlockedTimeOfDay
        );
    }

    #[test]
    fn per_day_window_overrides_default() {
        let mut p = ParentalPolicy::default();
        p.default_window = TimeWindow::always();
        p.per_day_window[3] = Some(TimeWindow {
            start_minute: 18 * 60,
            end_minute: 20 * 60,
        });
        assert_eq!(p.check(&ctx("videos", Rating::G, 3, 12 * 60)), Decision::BlockedTimeOfDay);
        assert_eq!(p.check(&ctx("videos", Rating::G, 3, 19 * 60)), Decision::Allow);
        assert_eq!(p.check(&ctx("videos", Rating::G, 2, 12 * 60)), Decision::Allow);
    }
}
