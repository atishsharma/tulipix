//! Desktop widgets — macOS WidgetKit, Windows 11 Widget Board, GNOME shell
//! extension. The Tulipix app exposes a content provider over a per-OS IPC
//! channel; each widget renders a fixed payload shape.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    NowPlaying,
    OnThisDay,
    RecentlyPlayed,
    WatchNext,
}

impl WidgetKind {
    pub fn deep_link(self) -> &'static str {
        match self {
            WidgetKind::NowPlaying     => "tulipix://music/now-playing",
            WidgetKind::OnThisDay      => "tulipix://photos/on-this-day",
            WidgetKind::RecentlyPlayed => "tulipix://music/history",
            WidgetKind::WatchNext      => "tulipix://videos/queue",
        }
    }
    pub fn refresh_seconds(self) -> u32 {
        match self {
            WidgetKind::NowPlaying     => 5,
            WidgetKind::OnThisDay      => 3600,
            WidgetKind::RecentlyPlayed => 60,
            WidgetKind::WatchNext      => 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub thumb_path: Option<String>,
    pub deep_link: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetPayload {
    pub kind: WidgetKind,
    pub items: Vec<WidgetItem>,
    pub generated_at: i64,
}

pub trait WidgetProvider: Send + Sync {
    fn payload(&self, kind: WidgetKind) -> WidgetPayload;
}

#[derive(Default)]
pub struct EmptyProvider;
impl WidgetProvider for EmptyProvider {
    fn payload(&self, kind: WidgetKind) -> WidgetPayload {
        WidgetPayload { kind, items: vec![], generated_at: 0 }
    }
}

/// Build the macOS WidgetKit timeline JSON (consumed by the SwiftUI widget
/// bundle that ships next to Tulipix.app).
pub fn render_widgetkit_timeline(p: &WidgetPayload) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "kind": p.kind,
        "generatedAt": p.generated_at,
        "entries": p.items.iter().map(|i| serde_json::json!({
            "title": i.title,
            "subtitle": i.subtitle,
            "thumb": i.thumb_path,
            "deepLink": i.deep_link,
        })).collect::<Vec<_>>(),
    })).unwrap_or_default()
}

/// Build the Windows 11 Widget Board adaptive-card JSON.
pub fn render_adaptive_card(p: &WidgetPayload) -> String {
    let body: Vec<_> = p.items.iter().map(|i| serde_json::json!({
        "type": "TextBlock", "text": i.title, "weight": "Bolder",
    })).collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "type": "AdaptiveCard", "version": "1.5", "body": body,
    })).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p() -> WidgetPayload {
        WidgetPayload {
            kind: WidgetKind::OnThisDay,
            items: vec![WidgetItem {
                title: "2019: Lake".into(), subtitle: Some("12 photos".into()),
                thumb_path: Some("/tmp/x.webp".into()),
                deep_link: "tulipix://photos/2019-05-20".into(),
            }],
            generated_at: 1747700000,
        }
    }
    #[test] fn deep_links_distinct() {
        let all = [WidgetKind::NowPlaying, WidgetKind::OnThisDay, WidgetKind::RecentlyPlayed, WidgetKind::WatchNext];
        let mut links: Vec<_> = all.iter().map(|k| k.deep_link()).collect();
        links.sort(); links.dedup();
        assert_eq!(links.len(), 4);
    }
    #[test] fn widgetkit_timeline_renders() {
        let s = render_widgetkit_timeline(&p());
        assert!(s.contains("on-this-day"));
        assert!(s.contains("2019: Lake"));
    }
    #[test] fn adaptive_card_has_body() {
        let s = render_adaptive_card(&p());
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "AdaptiveCard");
        assert!(!v["body"].as_array().unwrap().is_empty());
    }
}
