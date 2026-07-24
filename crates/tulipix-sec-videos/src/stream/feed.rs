//! The landing screen: hero, Continue Watching, and the catalogue's own rows.

use super::*;

/// How many cards each landing row carries.
const ROW_LEN: usize = 12;

/// One card, in a form that can cross a thread boundary.
///
/// `StreamCard` carries a `slint::Image`, which is not `Send`, so everything the
/// async side prepares is plain data and the widgets are built in the event loop.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardData {
    pub id: String,
    pub title: String,
    /// Second line: the year, or "S2E4 · 18 min left" on a resumable card.
    pub line: String,
    pub poster: Option<PathBuf>,
    pub is_series: bool,
    pub seasons: i32,
    /// 0.0..=1.0, drives the progress bar on a Continue Watching card.
    pub progress: f32,
}

impl CardData {
    fn of(hit: &stream::SearchHit, poster: Option<PathBuf>) -> Self {
        Self {
            id: hit.id.clone(),
            title: hit.title.clone(),
            line: hit.year.clone(),
            poster,
            is_series: hit.is_series,
            seasons: hit.season_subjects.len() as i32,
            progress: 0.0,
        }
    }

    fn card(&self) -> StreamCard {
        StreamCard {
            id: self.id.clone().into(),
            title: self.title.clone().into(),
            year: self.line.clone().into(),
            poster: load_image(self.poster.clone()),
            is_series: self.is_series,
            seasons: self.seasons,
            meta: Default::default(),
            overview: Default::default(),
            progress: self.progress,
        }
    }
}

pub(crate) fn cards_of(list: &[CardData]) -> Vec<StreamCard> {
    list.iter().map(CardData::card).collect()
}

/// Build the landing screen: Continue Watching first (it is the only row about
/// *you*), then whatever the catalogue is promoting.
///
/// Every part degrades on its own — no servers means the Continue Watching row
/// still paints from the local DB, and an unreadable feed leaves the browse rows
/// empty rather than blanking the page.
pub fn stream_feed_load(weak: slint::Weak<MainWindow>) {
    let epoch = next_epoch();
    tokio::runtime::Handle::current().spawn(async move {
        // 1. Local first: instant, and independent of the network.
        let resume = continue_watching_cards().await;
        if !is_current(epoch) {
            return;
        }
        push_hero(&weak, resume.first().cloned());
        push_rows(&weak, resume.clone(), Vec::new());

        // 2. Then the remote feed.
        let Ok(c) = client().await else {
            set_status(&weak, "No servers configured — open Servers and add one.", false);
            return;
        };
        let rows = c.feed().await.unwrap_or_default();
        if rows.is_empty() || !is_current(epoch) {
            return;
        }

        // Posters for every row in one pass, so the strips fill together.
        let trimmed: Vec<(String, Vec<stream::SearchHit>)> = rows
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                (stream::feed::row_label(r, idx), r.items.iter().take(ROW_LEN).cloned().collect())
            })
            .collect();
        let urls: Vec<String> =
            trimmed.iter().flat_map(|(_, items)| items.iter().map(|h| h.cover.clone())).collect();
        let mut covers = cache_covers(urls).await.into_iter();
        if !is_current(epoch) {
            return;
        }

        let browse: Vec<(String, Vec<CardData>)> = trimmed
            .into_iter()
            .map(|(title, items)| {
                let cards =
                    items.iter().map(|h| CardData::of(h, covers.next().flatten())).collect();
                (title, cards)
            })
            .collect();
        push_rows(&weak, resume, browse);
    });
}

/// Continue Watching as cards, newest first.
async fn continue_watching_cards() -> Vec<CardData> {
    let Ok(pool) = pool_for("videos").await else { return Vec::new() };
    let entries = stream::progress::continue_watching(&pool, ROW_LEN).await;
    if entries.is_empty() {
        return Vec::new();
    }
    let covers = cache_covers(entries.iter().map(|e| e.cover_url.clone()).collect()).await;
    entries
        .into_iter()
        .zip(covers)
        .map(|(e, poster)| CardData {
            id: e.subject_id.clone(),
            title: e.title.clone(),
            line: resume_label(&e),
            poster,
            is_series: e.is_series,
            seasons: 0,
            progress: e.fraction() as f32,
        })
        .collect()
}

/// "S02E04 · 18 min left" — the line under a Continue Watching card.
fn resume_label(e: &stream::progress::Entry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if e.is_series && (e.season > 0 || e.episode > 0) {
        parts.push(format!("S{:02}E{:02}", e.season.max(1), e.episode.max(1)));
    }
    match e.remaining_s() {
        Some(s) if s >= 60.0 => parts.push(format!("{:.0} min left", s / 60.0)),
        Some(_) => parts.push("nearly done".to_string()),
        None => {}
    }
    parts.join("  ·  ")
}

/// The hero: the title you were last watching, or nothing at all.
fn push_hero(weak: &slint::Weak<MainWindow>, card: Option<CardData>) {
    let _ = weak.upgrade_in_event_loop(move |w| match card {
        Some(c) => {
            w.set_video_stream_hero_id(c.id.clone().into());
            w.set_video_stream_hero_title(c.title.clone().into());
            w.set_video_stream_hero_meta(c.line.clone().into());
            w.set_video_stream_hero_progress(c.progress);
            w.set_video_stream_hero_cover(load_image(c.poster.clone()));
            w.set_video_stream_hero_open(true);
        }
        None => w.set_video_stream_hero_open(false),
    });
}

/// Publish the landing rows. `browse` is empty on the first, local-only pass.
fn push_rows(
    weak: &slint::Weak<MainWindow>,
    resume: Vec<CardData>,
    browse: Vec<(String, Vec<CardData>)>,
) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        let mut rows: Vec<StreamRow> = Vec::new();
        if !resume.is_empty() {
            rows.push(StreamRow {
                title: "Continue watching".into(),
                resumable: true,
                cards: slint::ModelRc::new(slint::VecModel::from(cards_of(&resume))),
            });
        }
        for (title, cards) in &browse {
            rows.push(StreamRow {
                title: title.clone().into(),
                resumable: false,
                cards: slint::ModelRc::new(slint::VecModel::from(cards_of(cards))),
            });
        }
        w.set_video_stream_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

/// Play the hero straight from the landing screen. Opening the title is enough:
/// the detail view resumes from `stream_progress` on its own.
pub fn stream_hero_play(weak: slint::Weak<MainWindow>, subject_id: String) {
    if !subject_id.is_empty() {
        stream_open(weak, subject_id);
    }
}

/// Forget one Continue Watching entry (its card's ✕).
pub fn stream_resume_forget(weak: slint::Weak<MainWindow>, subject_id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::progress::remove(&pool, &subject_id).await;
        }
        stream_feed_load(weak);
    });
}
