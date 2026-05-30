//! RTL layout policy. Drives the Slint mirror-flag per locale
//! (sidebar flips to right edge, scroll handles flip, semantically
//! directional icons mirror — chevrons, arrows, back/forward).
//!
//! Pure look-up table; layout components consume `TextDirection`
//! and `MirrorPolicy::should_mirror_icon(icon_id)` to decide.

use crate::i18n::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection { Ltr, Rtl }

/// Locales known to render RTL. Tulipix's initial set ships only LTR;
/// keep the list keyed on BCP-47 prefixes so adding `ar`/`he`/`fa`
/// flips layout the moment a locale bundle lands.
pub fn is_rtl_code(code: &str) -> bool {
    let lower = code.to_ascii_lowercase();
    let prefix = lower.split('-').next().unwrap_or("");
    matches!(prefix, "ar" | "he" | "iw" | "fa" | "ur" | "ps" | "yi" | "dv" | "sd")
}

pub fn direction_for(locale: Locale) -> TextDirection {
    if is_rtl_code(locale.code()) { TextDirection::Rtl } else { TextDirection::Ltr }
}

pub fn direction_for_code(code: &str) -> TextDirection {
    if is_rtl_code(code) { TextDirection::Rtl } else { TextDirection::Ltr }
}

/// Icons that carry direction. The Slint side calls
/// `MirrorPolicy::should_mirror_icon` with the icon id; non-directional
/// icons (heart, star, gear) always return false.
#[derive(Debug, Clone)]
pub struct MirrorPolicy { pub direction: TextDirection }

impl MirrorPolicy {
    pub fn for_code(code: &str) -> Self { Self { direction: direction_for_code(code) } }
    pub fn should_mirror_layout(&self) -> bool { self.direction == TextDirection::Rtl }
    pub fn should_mirror_icon(&self, icon_id: &str) -> bool {
        if self.direction == TextDirection::Ltr { return false; }
        matches!(icon_id, "chevron-left" | "chevron-right" | "arrow-left" | "arrow-right"
                       | "back" | "forward" | "next" | "previous" | "indent" | "outdent"
                       | "play" | "rewind" | "skip-back" | "skip-forward")
    }
}

/// Headless test helper — given a node tree of (id, x), flips x
/// around `width` for RTL. Layout components use the same pivot.
pub fn flip_x(direction: TextDirection, x: f32, width: f32, content_width: f32) -> f32 {
    match direction {
        TextDirection::Ltr => x,
        TextDirection::Rtl => width - x - content_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detects_rtl_prefix() {
        assert!(is_rtl_code("ar"));
        assert!(is_rtl_code("ar-EG"));
        assert!(is_rtl_code("he-IL"));
        assert!(is_rtl_code("FA"));
        assert!(!is_rtl_code("en"));
        assert!(!is_rtl_code("zh-CN"));
        assert!(!is_rtl_code("hi"));
    }
    #[test] fn current_locales_all_ltr() {
        for &l in Locale::all() { assert_eq!(direction_for(l), TextDirection::Ltr, "{}", l.code()); }
    }
    #[test] fn mirror_policy_branches() {
        let m = MirrorPolicy::for_code("ar");
        assert!(m.should_mirror_layout());
        assert!(m.should_mirror_icon("chevron-left"));
        assert!(m.should_mirror_icon("play"));
        assert!(!m.should_mirror_icon("heart"));
        let l = MirrorPolicy::for_code("en");
        assert!(!l.should_mirror_layout());
        assert!(!l.should_mirror_icon("chevron-left"));
    }
    #[test] fn flip_x_pivots_around_width() {
        // Headless layout test: a 100-wide child at x=10 in a 400-wide row
        // ends up at 290 in RTL.
        assert_eq!(flip_x(TextDirection::Ltr, 10.0, 400.0, 100.0), 10.0);
        assert_eq!(flip_x(TextDirection::Rtl, 10.0, 400.0, 100.0), 290.0);
    }
}
