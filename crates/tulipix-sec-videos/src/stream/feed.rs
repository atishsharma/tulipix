//! The landing row: the resume card, and a few picks from the catalogue's feed.

use super::*;

/// The resume slider holds this many of the most recent unfinished titles;
/// everything older is what the History page is for.
const RESUME_LEN: usize = 5;

/// One card, in a form that can cross a thread boundary.
///
/// `StreamCard` carries a `slint::Image`, which is not `Send`, so everything the
/// async side prepares is plain data and the widgets are built in the event loop.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardData {
    pub id: String,
    pub title: String,
    /// Second line: the year, or "S02E04" on a resumable card.
    pub line: String,
    /// Resume cards only: "18 min left".
    pub meta: String,
    /// Resume cards only: "34% watched".
    pub note: String,
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
            meta: String::new(),
            note: String::new(),
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
            meta: self.meta.clone().into(),
            overview: self.note.clone().into(),
            progress: self.progress,
        }
    }
}

pub(crate) fn cards_of(list: &[CardData]) -> Vec<StreamCard> {
    list.iter().map(CardData::card).collect()
}

/// How many trending picks sit under the search box. One short row — the
/// landing screen is a search page first.
const TRENDING_LEN: usize = 6;
/// How many vertical dramas the left-hand slider cycles through.
const VERTICAL_LEN: usize = 10;

/// Build the landing row: where you left off, then a few things worth opening.
///
/// The picks come from the DB first and are only refetched once they are half a
/// day old — the browse feed is an editorial front page, so asking for it on
/// every tab visit spends a request and a poster batch to arrive at the same six
/// cards. Both halves degrade on their own: no servers still leaves the resume
/// slider (it is local), and a failed refresh keeps showing the stored picks.
pub fn stream_feed_load(weak: slint::Weak<MainWindow>) {
    let epoch = next_feed_epoch();
    tokio::runtime::Handle::current().spawn(async move {
        // 1. Local first: instant, and independent of the network.
        let resume = continue_watching_cards().await;
        if !is_current_feed(epoch) {
            return;
        }
        push_resume(&weak, resume);

        let pool = pool_for("videos").await.ok();
        let stored = match &pool {
            Some(p) => (
                stream::feed_cache::load(p, KIND_TRENDING).await,
                stream::feed_cache::load(p, KIND_VERTICAL).await,
            ),
            None => (None, None),
        };
        // 2. Whatever is stored paints straight away.
        if let Some(c) = &stored.0 {
            paint_picks(&weak, epoch, c.hits.clone(), push_trending).await;
        }
        if let Some(c) = &stored.1 {
            paint_picks(&weak, epoch, c.hits.clone(), push_vertical).await;
        }

        // 3. Only go to the servers when the picks have aged out.
        let stale = stream::feed_cache::needs_refresh(stored.0.as_ref())
            || stream::feed_cache::needs_refresh(stored.1.as_ref());
        if !stale || !is_current_feed(epoch) {
            return;
        }
        let Ok(c) = client().await else {
            if stored.0.is_none() {
                set_status(&weak, "No servers configured — open Servers and add one.", false);
            }
            return;
        };
        let rows = c.feed().await.unwrap_or_default();
        if rows.is_empty() || !is_current_feed(epoch) {
            return; // a failed refresh leaves the stored picks on screen
        }
        let picks = trending_picks(&rows);
        let verticals = vertical_picks(&rows, &picks);
        if let Some(p) = &pool {
            let _ = stream::feed_cache::store(p, KIND_TRENDING, &picks).await;
            let _ = stream::feed_cache::store(p, KIND_VERTICAL, &verticals).await;
        }
        paint_picks(&weak, epoch, picks, push_trending).await;
        paint_picks(&weak, epoch, verticals, push_vertical).await;
    });
}

/// Open a title against a named catalogue rather than whichever one the provider
/// button is showing.
///
/// The landing screen is the one place where cards on screen did not come from
/// the active source, so opening one there has to say which catalogue its id
/// belongs to. The two id namespaces are unrelated — a MovieBox subject id means
/// nothing to 4KHDHub — so getting this wrong is an empty detail page, not a
/// wrong-looking one.
fn open_against(weak: slint::Weak<MainWindow>, subject_id: String, source: Source) {
    // A no-op when it already matches. It clears the open title, so it has to
    // run before the open, not after.
    if source != active_source() {
        stream_set_source(weak.clone(), source.key().to_string());
    }
    stream_open(weak, subject_id);
}

/// Open a trending or vertical-drama pick.
///
/// Both lists are built from MovieBox's feed whatever the provider button says
/// — `stream_feed_load` goes to `client()` directly and never consults
/// `catalogue()` — so their ids only ever mean something to MovieBox.
pub fn stream_open_pick(weak: slint::Weak<MainWindow>, subject_id: String) {
    if subject_id.is_empty() {
        return;
    }
    open_against(weak, subject_id, Source::MovieBox);
}

/// Open a Continue Watching card against the catalogue it was actually played
/// from.
///
/// The resume row mixes both sources — one progress table, and playback records
/// into it whichever catalogue was in use. A row with no recorded source
/// (written before the column existed) opens against the current selection,
/// which is what it did before.
pub fn stream_open_resume(weak: slint::Weak<MainWindow>, subject_id: String) {
    if subject_id.is_empty() {
        return;
    }
    tokio::runtime::Handle::current().spawn(async move {
        let recorded = match pool_for("videos").await {
            Ok(pool) => stream::progress::source_of(&pool, &subject_id).await,
            Err(_) => None,
        };
        let source = recorded.map_or_else(active_source, |k| Source::parse(&k));
        open_against(weak, subject_id, source);
    });
}

/// Cache keys for the two landing lists.
const KIND_TRENDING: &str = "trending";
const KIND_VERTICAL: &str = "vertical";

/// Cache the posters for one list, then hand it to `publish`.
async fn paint_picks(
    weak: &slint::Weak<MainWindow>,
    epoch: u64,
    hits: Vec<stream::SearchHit>,
    publish: fn(&slint::Weak<MainWindow>, Vec<CardData>),
) {
    if hits.is_empty() {
        return;
    }
    let covers = cache_covers(hits.iter().map(|h| h.cover.clone()).collect()).await;
    if !is_current_feed(epoch) {
        return;
    }
    let cards: Vec<CardData> =
        hits.iter().zip(covers).map(|(hit, poster)| CardData::of(hit, poster)).collect();
    publish(weak, cards);
}

/// There is no documented flag for the short, phone-shaped serials, so both the
/// trending row and the vertical slider go by the heading the catalogue files
/// them under. One list, read by both, so the two cannot drift apart.
fn is_short_form(row: &stream::feed::FeedRow) -> bool {
    const MARKERS: [&str; 5] = ["short", "vertical", "mini", "quick", "drama"];
    let heading = row.title.to_lowercase();
    MARKERS.iter().any(|m| heading.contains(m))
}

/// Six titles off the top of the feed, series first.
///
/// Series are preferred because the feed's movie entries come through without
/// usable artwork often enough that a row of them reads as broken; a movie only
/// appears here when there are not six series to show.
fn trending_picks(rows: &[stream::feed::FeedRow]) -> Vec<stream::SearchHit> {
    // Short-form rows are left out: the vertical slider below draws from them,
    // and trending padding itself to six would take their titles first, leaving
    // that slider empty on exactly the small feeds where it has least to spare.
    // If the feed is nothing but short-form rows, a trending row of them still
    // beats no trending row at all.
    let mut pool: Vec<&stream::feed::FeedRow> = rows.iter().filter(|r| !is_short_form(r)).collect();
    if pool.is_empty() {
        pool = rows.iter().collect();
    }
    let flat: Vec<&stream::SearchHit> = pool.iter().flat_map(|r| r.items.iter()).collect();
    let mut out: Vec<stream::SearchHit> = Vec::new();
    let take = |keep: &dyn Fn(&stream::SearchHit) -> bool, out: &mut Vec<stream::SearchHit>| {
        for hit in flat.iter().filter(|h| keep(h)) {
            if out.len() >= TRENDING_LEN {
                break;
            }
            if !out.iter().any(|o| o.id == hit.id) {
                out.push((*hit).clone());
            }
        }
    };
    // Artwork is not optional here: a card without it is a grey box with a
    // title, which is what "trending is broken" looked like. A short row is the
    // better failure.
    take(&|h| h.is_series && !h.cover.is_empty(), &mut out);
    take(&|h| !h.cover.is_empty(), &mut out);
    out
}

/// Vertical dramas — the short, phone-shaped serials the catalogue runs as their
/// own strand.
///
/// There is no documented flag for them, so this goes by the row they are filed
/// under: headings mentioning short-form or vertical. When the feed uses none of
/// those words it falls back to titles the trending row did not take, which is
/// wrong-but-harmless — the slider still shows something openable.
fn vertical_picks(
    rows: &[stream::feed::FeedRow],
    taken: &[stream::SearchHit],
) -> Vec<stream::SearchHit> {
    let spoken_for = |h: &stream::SearchHit| taken.iter().any(|t| t.id == h.id);

    let mut out: Vec<stream::SearchHit> = Vec::new();
    let push = |hit: &stream::SearchHit, out: &mut Vec<stream::SearchHit>| {
        if out.len() < VERTICAL_LEN
            && !hit.cover.is_empty()
            && !spoken_for(hit)
            && !out.iter().any(|o| o.id == hit.id)
        {
            out.push(hit.clone());
        }
    };
    for row in rows.iter().filter(|r| is_short_form(r)) {
        for hit in &row.items {
            push(hit, &mut out);
        }
    }
    // Nothing filed that way — fall back to whatever the trending row left.
    if out.is_empty() {
        for hit in rows.iter().flat_map(|r| r.items.iter()) {
            push(hit, &mut out);
        }
    }
    out
}

fn push_vertical(weak: &slint::Weak<MainWindow>, cards: Vec<CardData>) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_vertical(slint::ModelRc::new(slint::VecModel::from(cards_of(&cards))));
    });
}

fn push_trending(weak: &slint::Weak<MainWindow>, cards: Vec<CardData>) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_trending(slint::ModelRc::new(slint::VecModel::from(cards_of(&cards))));
    });
}

/// Continue Watching as cards, newest first.
async fn continue_watching_cards() -> Vec<CardData> {
    let Ok(pool) = pool_for("videos").await else { return Vec::new() };
    let entries = stream::progress::continue_watching(&pool, RESUME_LEN).await;
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
            line: episode_label(&e),
            meta: left_label(&e),
            note: format!("{:.0}% watched", e.fraction() * 100.0),
            poster,
            is_series: e.is_series,
            seasons: 0,
            progress: e.fraction() as f32,
        })
        .collect()
}

/// Which episode you are on: "S02E04", or the year for a film.
fn episode_label(e: &stream::progress::Entry) -> String {
    if e.is_series && (e.season > 0 || e.episode > 0) {
        format!("Season {}  ·  Episode {}", e.season.max(1), e.episode.max(1))
    } else {
        String::new()
    }
}

/// How much of it is left: "18 min left".
fn left_label(e: &stream::progress::Entry) -> String {
    match e.remaining_s() {
        Some(s) if s >= 3600.0 => format!("{:.0} h {:02.0} min left", s / 3600.0, (s % 3600.0) / 60.0),
        Some(s) if s >= 60.0 => format!("{:.0} min left", s / 60.0),
        Some(_) => "nearly done".to_string(),
        None => String::new(),
    }
}

/// The resume slider's cards — newest first, empty when nothing is part-watched.
fn push_resume(weak: &slint::Weak<MainWindow>, cards: Vec<CardData>) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_resume(slint::ModelRc::new(slint::VecModel::from(cards_of(&cards))));
    });
}


#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, series: bool, cover: bool) -> stream::SearchHit {
        stream::SearchHit {
            id: id.into(),
            title: format!("Title {id}"),
            year: "2024".into(),
            cover: if cover { format!("https://img/{id}.jpg") } else { String::new() },
            is_series: series,
            season_subjects: Vec::new(),
        }
    }

    fn row(title: &str, items: Vec<stream::SearchHit>) -> stream::feed::FeedRow {
        stream::feed::FeedRow { title: title.into(), items }
    }

    #[test]
    fn trending_prefers_series_and_never_pads_with_artless_cards() {
        let rows = vec![row(
            "Popular",
            vec![
                hit("m1", false, true),
                hit("s1", true, true),
                hit("s2", true, true),
                hit("blank", true, false),
            ],
        )];
        let picks = trending_picks(&rows);
        // Series first, then the film; the artwork-less entry is left out even
        // though the row is short of six.
        assert_eq!(
            picks.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["s1", "s2", "m1"]
        );
    }

    #[test]
    fn verticals_come_from_short_form_rows_and_skip_what_trending_took() {
        let rows = vec![
            row("Trending now", vec![hit("s1", true, true), hit("s2", true, true)]),
            row("Short dramas", vec![hit("s2", true, true), hit("v1", true, true)]),
        ];
        let picks = trending_picks(&rows);
        let verticals = vertical_picks(&rows, &picks);
        // s2 is already on the trending row, so the slider does not repeat it.
        assert_eq!(verticals.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["v1"]);
    }

    #[test]
    fn trending_takes_short_form_rows_only_when_they_are_all_there_is() {
        let rows = vec![row("Short dramas", vec![hit("v1", true, true), hit("v2", true, true)])];
        assert_eq!(
            trending_picks(&rows).iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["v1", "v2"]
        );
    }

    #[test]
    fn verticals_fall_back_when_no_row_is_labelled_short_form() {
        let rows = vec![row(
            "Trending now",
            vec![hit("s1", true, true), hit("s2", true, true), hit("extra", true, true)],
        )];
        let picks = trending_picks(&rows);
        let verticals = vertical_picks(&rows, &picks);
        // Everything is spoken for by trending, so the slider stays empty rather
        // than repeating the row beside it.
        assert!(verticals.is_empty(), "{verticals:#?}");
    }
}
