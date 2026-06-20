//! Videos-section library extracted from tulipix-app: grid/show models,
//! season/episode parsing, TMDB poster scraping, Discover, and the per-tab row
//! queries. The embedded mpv player stays in main; the video `window.on_*`
//! callbacks there reach these via `use tulipix_sec_videos::*`.

use std::path::PathBuf;
use tulipix_ui::*;
use tulipix_common::*;

// Map video tile index → absolute path so a click can hand the file to mpv.
// Rebuilt by kick_video_refresh to match whatever tab is on screen.
pub static VIDEO_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
pub fn video_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    VIDEO_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Map video tile index → DB item id (parallel to video_paths), for flag actions.
pub static VIDEO_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn video_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    VIDEO_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Accumulated scan output for the videos section: (label, abs_path, thumb_path).
// kick_video_refresh joins this against the DB so a tab switch re-orders without
// re-rendering any thumbnails.
type VideoEntry = (String, PathBuf, PathBuf);
pub static VIDEO_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<VideoEntry>>> = std::sync::OnceLock::new();
pub fn video_full() -> &'static std::sync::Mutex<Vec<VideoEntry>> {
    VIDEO_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Currently drilled-into TV show (None = showing show cards / not in a show).
pub static VIDEO_SHOW: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
pub fn video_show() -> &'static std::sync::Mutex<Option<i64>> {
    VIDEO_SHOW.get_or_init(|| std::sync::Mutex::new(None))
}

/// Parse a `SxxEyy` / `SxxExx` season+episode out of a filename. Tolerant of
/// `s01e01`, `S1E1`, `S01.E01`. Returns (season, episode).
pub fn parse_season_episode(name: &str) -> Option<(i64, i64)> {
    let b = name.as_bytes();
    let lower = name.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut i = 0;
    while i < lb.len() {
        if lb[i] == b's' {
            // read up to 2 digits for season
            let mut j = i + 1;
            let s0 = j;
            while j < lb.len() && lb[j].is_ascii_digit() && j - s0 < 2 { j += 1; }
            if j == s0 { i += 1; continue; }
            let season: i64 = lower[s0..j].parse().ok()?;
            // optional separator then 'e'
            let mut k = j;
            while k < lb.len() && (lb[k] == b'.' || lb[k] == b' ' || lb[k] == b'_' || lb[k] == b'-') { k += 1; }
            if k < lb.len() && lb[k] == b'e' {
                let e0 = k + 1;
                let mut e = e0;
                while e < lb.len() && lb[e].is_ascii_digit() && e - e0 < 3 { e += 1; }
                if e > e0 {
                    let episode: i64 = lower[e0..e].parse().ok()?;
                    let _ = b; // keep original for potential future use
                    return Some((season, episode));
                }
            }
        }
        i += 1;
    }
    None
}

/// Fetch the stored TMDB API key from keychain (None → TMDB scraping disabled).
pub fn tmdb_api_key() -> Option<String> {
    tulipix_core::api_keys::fetch("tmdb").ok().flatten()
}

/// Local poster cache directory: ~/.cache/tulipix/videos/posters/
pub fn poster_cache_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("posters"))
}

/// Background TMDB scrape for a freshly-scanned video file.
/// Detects movie vs TV episode, fetches metadata + poster, stores locally.
/// Fire-and-forget: caller spawns this as a separate task.
pub async fn scrape_video_tmdb(pool: sqlx::SqlitePool, item_id: i64, path: PathBuf) {
    let Some(api_key) = tmdb_api_key() else { return; };
    let Some(cache_dir) = poster_cache_dir() else { return; };
    let client = tulipix_videos::tmdb::TmdbClient::new(api_key);
    // Determine if this item has been classified as a TV episode.
    let is_episode: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM episodes WHERE item_id = ?")
        .bind(item_id).fetch_one(&pool).await.unwrap_or(0) > 0;
    if is_episode {
        scrape_show_for_item(&pool, item_id, &client, &cache_dir).await;
    } else {
        scrape_movie_for_item(&pool, item_id, &path, &client, &cache_dir).await;
    }
}

/// Parse filename → TMDB movie search → upsert movies + cache poster.
pub async fn scrape_movie_for_item(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    path: &std::path::Path,
    client: &tulipix_videos::tmdb::TmdbClient,
    cache_dir: &std::path::Path,
) {
    use tulipix_videos::tmdb::MetadataProvider as _;
    // Skip if already scraped.
    let cached: Option<String> = sqlx::query_scalar("SELECT poster_local FROM movies WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await.ok().flatten().flatten();
    if cached.is_some() { return; }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parsed = tulipix_videos::agents::parse_filename(name);
    let Ok(Some(meta)) = client.search_movie(&parsed.title, parsed.year).await else { return; };
    let _ = tulipix_videos::tmdb::upsert_movie(pool, item_id, &meta).await;
    if let Some(pp) = &meta.poster_path {
        if let Ok(local) = client.cache_poster(pp, cache_dir).await {
            let _ = sqlx::query("UPDATE movies SET poster_local = ? WHERE item_id = ?")
                .bind(local.to_string_lossy().as_ref()).bind(item_id).execute(pool).await;
        }
    }
}

/// Look up the parent show → TMDB TV search → update shows row + cache poster.
pub async fn scrape_show_for_item(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    client: &tulipix_videos::tmdb::TmdbClient,
    cache_dir: &std::path::Path,
) {
    use tulipix_videos::tmdb::MetadataProvider as _;
    let Ok(Some(show_id)) = sqlx::query_scalar::<_, i64>(
        "SELECT show_id FROM episodes WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await else { return; };
    // Skip if already scraped.
    let cached: Option<String> = sqlx::query_scalar("SELECT poster_local FROM shows WHERE id = ?")
        .bind(show_id).fetch_optional(pool).await.ok().flatten().flatten();
    if cached.is_some() { return; }
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM shows WHERE id = ?")
        .bind(show_id).fetch_optional(pool).await.ok().flatten();
    let Some(title) = title else { return; };
    let Ok(Some(meta)) = client.search_show(&title).await else { return; };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // Update the existing show row in-place (avoids duplicate rows).
    let _ = sqlx::query(
        "UPDATE shows SET tmdb_id=?, tvdb_id=?, year=?, overview=?, \
         poster_path=?, backdrop_path=?, updated=? WHERE id=?")
        .bind(meta.tmdb_id).bind(meta.tvdb_id).bind(meta.year).bind(&meta.overview)
        .bind(&meta.poster_path).bind(&meta.backdrop_path).bind(now).bind(show_id)
        .execute(pool).await;
    if let Some(pp) = &meta.poster_path {
        if let Ok(local) = client.cache_poster(pp, cache_dir).await {
            let _ = sqlx::query("UPDATE shows SET poster_local = ? WHERE id = ?")
                .bind(local.to_string_lossy().as_ref()).bind(show_id).execute(pool).await;
        }
    }
}

/// Get-or-create a local `shows` row by title (no TMDB/TVDB id), returning id.
pub async fn get_or_create_show(pool: &sqlx::SqlitePool, title: &str) -> Option<i64> {
    if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title).fetch_optional(pool).await { return Some(id); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    sqlx::query("INSERT INTO shows (title, updated) VALUES (?, ?)")
        .bind(title).bind(now).execute(pool).await.ok()?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title).fetch_optional(pool).await.ok().flatten()
}

/// If `path`'s filename carries an SxxEyy tag, link it as an episode of the
/// show named by its parent directory.
pub async fn classify_tv_episode(pool: &sqlx::SqlitePool, item_id: i64, path: &std::path::Path) {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else { return; };
    let Some((season, episode)) = parse_season_episode(name) else { return; };
    let show_title = path.parent()
        .and_then(|p| p.file_name()).and_then(|s| s.to_str())
        .unwrap_or("Unknown Show").to_string();
    let Some(show_id) = get_or_create_show(pool, &show_title).await else { return; };
    let _ = tulipix_videos::episodes::upsert(
        pool, item_id, show_id, season, episode, None, None, None, None, None).await;
}

/// One TV show card for the Videos "TV" tab.
pub struct ShowRow { id: i64, title: String, count: i64, cover_path: String, poster_local: Option<String> }

/// Load show cards (title + episode count + poster; falls back to first episode's abs_path).
pub async fn video_show_cards(pool: &sqlx::SqlitePool) -> Vec<ShowRow> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.id, s.title, COUNT(e.item_id) AS n, \
                (SELECT i.abs_path FROM episodes e2 JOIN items i ON i.id = e2.item_id \
                 WHERE e2.show_id = s.id ORDER BY e2.season, e2.episode LIMIT 1) AS cover, \
                s.poster_local \
         FROM shows s JOIN episodes e ON e.show_id = s.id \
         GROUP BY s.id ORDER BY s.title",
    ).fetch_all(pool).await.unwrap_or_default();
    rows.into_iter().map(|(id, title, count, cover, poster_local)| ShowRow {
        id, title, count, cover_path: cover.unwrap_or_default(), poster_local,
    }).collect()
}

// Top-level video filter (tv|movies|local) + search query — read by video_rows_for.
pub static VIDEO_KIND: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn video_kind() -> &'static std::sync::Mutex<String> {
    VIDEO_KIND.get_or_init(|| std::sync::Mutex::new("local".into()))
}
pub static VIDEO_QUERY: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn video_query() -> &'static std::sync::Mutex<String> {
    VIDEO_QUERY.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// A single video library row joined across video_meta + watch_progress.
pub struct VideoRow {
    abs_path: String,
    item_id: i64,
    starred: bool,
    watched: bool,
    progress: f32,
    duration: String,
    season: i32,
    episode: i32,
    poster_local: Option<String>,  // TMDB-cached poster abs path (np.p3.tmdb)
}

/// Query the videos DB for the rows that belong to `category`, already ordered
/// the way the tab wants to show them.
pub async fn video_rows_for(pool: &sqlx::SqlitePool, category: &str) -> Vec<VideoRow> {
    let (filter, mut order): (&str, String) = match category {
        "continue" => (
            "vm.deleted_at IS NULL AND vm.archived = 0 AND vm.last_accessed IS NOT NULL \
             AND COALESCE(wp.finished,0) = 0 AND COALESCE(wp.position_s,0) > 0",
            "vm.last_accessed DESC".into(),
        ),
        "starred" => ("vm.deleted_at IS NULL AND vm.starred = 1", "i.added DESC, i.id DESC".into()),
        "archive" => ("vm.deleted_at IS NULL AND vm.archived = 1", "i.added DESC, i.id DESC".into()),
        "trash"   => ("vm.deleted_at IS NOT NULL", "vm.deleted_at DESC".into()),
        _         => ("vm.deleted_at IS NULL AND vm.archived = 0", "i.added DESC, i.id DESC".into()),
    };
    // Top-level kind (tv|movies|local). Movies restricts to scraped movies;
    // TV restricts to episodes (and, when drilled into a show, to that show,
    // ordered by season/episode); local shows everything in the section.
    let kind = video_kind().lock().map(|g| g.clone()).unwrap_or_else(|_| "local".into());
    // `episodes` is always joined so season/episode are available for the
    // grouped browser; the tv kind_clause restricts to actual episodes.
    let kind_clause: String = match kind.as_str() {
        "movies" => " AND vm.item_id IN (SELECT item_id FROM movies)".into(),
        "tv" => {
            order = "ep.season, ep.episode".into();
            match *video_show().lock().unwrap_or_else(|p| p.into_inner()) {
                Some(id) => format!(" AND ep.show_id = {id}"),
                None     => " AND ep.item_id IS NOT NULL".into(),
            }
        }
        _ => String::new(),
    };
    let sql = format!(
        "SELECT i.abs_path, vm.item_id, vm.starred, \
                COALESCE(wp.finished,0) AS finished, \
                COALESCE(wp.position_s, 0.0) AS pos, \
                COALESCE(wp.duration_s, vm.duration_s) AS dur, \
                COALESCE(ep.season, 0) AS season, \
                COALESCE(ep.episode, 0) AS episode, \
                mv.poster_local AS poster_local \
         FROM video_meta vm \
         JOIN items i ON i.id = vm.item_id \
         LEFT JOIN watch_progress wp ON wp.item_id = vm.item_id \
         LEFT JOIN episodes ep ON ep.item_id = vm.item_id \
         LEFT JOIN movies mv ON mv.item_id = vm.item_id \
         WHERE {filter}{kind_clause} ORDER BY {order}",
    );
    let rows: Vec<(String, i64, i64, i64, f64, Option<f64>, i64, i64, Option<String>)> =
        sqlx::query_as(&sql).fetch_all(pool).await.unwrap_or_default();
    let q = video_query().lock().map(|g| g.to_lowercase()).unwrap_or_default();
    rows.into_iter().filter(|(abs_path, ..)| {
        // Filename search filter (case-insensitive substring).
        q.is_empty() || std::path::Path::new(abs_path).file_name()
            .and_then(|s| s.to_str()).map(|n| n.to_lowercase().contains(&q)).unwrap_or(false)
    }).map(|(abs_path, item_id, starred, finished, pos, dur, season, episode, poster_local)| {
        let progress = match dur {
            Some(d) if d > 0.0 => (pos / d).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        };
        VideoRow {
            abs_path, item_id,
            starred: starred != 0,
            watched: finished != 0,
            progress,
            duration: dur.map(fmt_duration).unwrap_or_default(),
            season: season as i32,
            episode: episode as i32,
            poster_local,
        }
    }).collect()
}

/// Format a TMDB `release_date` ("YYYY-MM-DD") → human-readable "Jun 14, 2025".
pub fn fmt_release_date(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() < 10 { return s.to_string(); }
    let month = match &s[5..7] {
        "01" => "Jan", "02" => "Feb", "03" => "Mar", "04" => "Apr",
        "05" => "May", "06" => "Jun", "07" => "Jul", "08" => "Aug",
        "09" => "Sep", "10" => "Oct", "11" => "Nov", "12" => "Dec",
        _ => return s.to_string(),
    };
    let day: i32 = s[8..10].parse().unwrap_or(0);
    let year = &s[0..4];
    format!("{month} {day}, {year}")
}

/// Load the four TMDB discover rails from the DB cache, download any missing
/// posters, then push all four models to the UI thread. Kicks a background
/// TMDB refresh so the next open of the Discover tab is always fresh.
pub fn kick_discover_refresh(weak: slint::Weak<MainWindow>) {
    use tulipix_videos::discover::{DiscoverKind, TmdbDiscover, load_feed, refresh};
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return; };
        let api_key = tmdb_api_key();
        let cache_dir = poster_cache_dir();
        // Helper: load one kind, optionally trigger a background TMDB refresh
        // when the key is available.
        async fn load_kind(
            pool: &sqlx::SqlitePool,
            kind: DiscoverKind,
            api_key: Option<&str>,
            _cache_dir: Option<&std::path::Path>,
        ) -> Vec<tulipix_videos::discover::DiscoverItem> {
            if let Some(key) = api_key {
                let client = TmdbDiscover::new(key);
                // Refresh in background — don't await; serve from cache below.
                let _ = refresh(pool, &client, kind).await;
            }
            load_feed(pool, kind).await.unwrap_or_default()
        }
        // Run all four kinds concurrently.
        let (trending_movies, trending_shows, upcoming, on_air) = tokio::join!(
            load_kind(&pool, DiscoverKind::TrendingMoviesDay, api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::TrendingShowsDay,  api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::UpcomingMovies,    api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::OnTheAirShows,     api_key.as_deref(), cache_dir.as_deref()),
        );

        // For each item, cache the poster locally and return a DiscoverCard.
        fn build_cards(
            items: Vec<tulipix_videos::discover::DiscoverItem>,
            cache_dir: Option<&std::path::Path>,
        ) -> Vec<(tulipix_videos::discover::DiscoverItem, Option<PathBuf>)> {
            items.into_iter().map(|it| {
                let local = it.poster_path.as_deref().and_then(|pp| {
                    cache_dir.and_then(|dir| {
                        // Check if already cached (sha256 path = same formula as cache_image).
                        use sha2::{Digest, Sha256};
                        let url = format!("https://image.tmdb.org/t/p/w500{}", pp);
                        let mut h = Sha256::new();
                        h.update(url.as_bytes());
                        let name = format!("{:x}.jpg", h.finalize());
                        let p = dir.join(name);
                        if p.exists() { Some(p) } else { None }
                    })
                });
                (it, local)
            }).collect()
        }
        let cd = cache_dir.as_deref();
        let trending_movies_cards = build_cards(trending_movies, cd);
        let trending_shows_cards  = build_cards(trending_shows, cd);
        let upcoming_cards        = build_cards(upcoming, cd);
        let on_air_cards          = build_cards(on_air, cd);

        let _ = weak.upgrade_in_event_loop(move |w| {
            fn to_slint(pairs: Vec<(tulipix_videos::discover::DiscoverItem, Option<PathBuf>)>) -> Vec<DiscoverCard> {
                pairs.into_iter().map(|(it, local)| {
                    let poster = local
                        .as_deref()
                        .and_then(|p| slint::Image::load_from_path(p).ok())
                        .unwrap_or_default();
                    let release = it.release_date.as_deref()
                        .map(fmt_release_date).unwrap_or_default();
                    DiscoverCard {
                        tmdb_id: it.tmdb_id as i32,
                        title: it.title.into(),
                        year: it.year.unwrap_or(0) as i32,
                        poster,
                        vote: it.vote_average.unwrap_or(0.0) as f32,
                        release_date: release.into(),
                        is_movie: it.is_movie,
                    }
                }).collect()
            }
            w.set_video_discover_movies(slint::ModelRc::new(slint::VecModel::from(to_slint(trending_movies_cards))));
            w.set_video_discover_shows(slint::ModelRc::new(slint::VecModel::from(to_slint(trending_shows_cards))));
            w.set_video_discover_upcoming(slint::ModelRc::new(slint::VecModel::from(to_slint(upcoming_cards))));
            w.set_video_discover_on_air(slint::ModelRc::new(slint::VecModel::from(to_slint(on_air_cards))));
        });

        // Now download any missing posters in the background and refresh models.
        if let (Some(key), Some(dir)) = (api_key.as_deref(), cache_dir.as_deref()) {
            let client = tulipix_videos::tmdb::TmdbClient::new(key);
            // Collect all items that need poster download.
            // Re-load all four kinds from DB to get their poster_paths.
            let all_items = {
                let mut v = Vec::new();
                for kind in [DiscoverKind::TrendingMoviesDay, DiscoverKind::TrendingShowsDay,
                              DiscoverKind::UpcomingMovies, DiscoverKind::OnTheAirShows] {
                    if let Ok(items) = load_feed(&pool, kind).await {
                        v.extend(items);
                    }
                }
                v
            };
            let dir_owned = dir.to_path_buf();
            tokio::spawn(async move {
                for it in all_items {
                    if let Some(pp) = &it.poster_path {
                        let _ = client.cache_poster(pp, &dir_owned).await;
                    }
                }
            });
        }
    });
}

/// Rebuild the videos grid for `category` off-thread, then set the tiles +
/// index→path/id maps on the UI thread. Thumbnails come from the accumulated
/// scan output (video_full) so a tab switch never re-renders a frame.
pub fn kick_video_refresh(weak: slint::Weak<MainWindow>, category: String) {
    let handle = tokio::runtime::Handle::current();
    // TV tab, not drilled into a show, on the Library sub-tab → show cards.
    let kind = video_kind().lock().map(|g| g.clone()).unwrap_or_else(|_| "local".into());
    let drilled = video_show().lock().map(|g| g.is_some()).unwrap_or(false);
    let tv_cards = kind == "tv" && !drilled && category == "library";
    if tv_cards {
        handle.spawn(async move {
            let (cards, next_up) = match pool_for("videos").await {
                Ok(pool) => (
                    video_show_cards(&pool).await,
                    tulipix_videos::episodes::next_up(&pool, 12).await.unwrap_or_default(),
                ),
                Err(_) => (Vec::new(), Vec::new()),
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                let by_path: std::collections::HashMap<String, PathBuf> = video_full()
                    .lock().map(|g| g.iter()
                        .map(|(_, orig, thumb)| (orig.to_string_lossy().into_owned(), thumb.clone()))
                        .collect())
                    .unwrap_or_default();
                let shows: Vec<ShowCard> = cards.into_iter().map(|s| {
                    // Prefer TMDB cached poster (np.p3.tmdb); fall back to
                    // ffmpeg scan thumb, then the raw episode abs path.
                    let thumb = s.poster_local.as_deref()
                        .map(PathBuf::from)
                        .filter(|p| p.exists())
                        .or_else(|| by_path.get(&s.cover_path).cloned())
                        .unwrap_or_else(|| PathBuf::from(&s.cover_path));
                    ShowCard {
                        id: s.id as i32,
                        title: s.title.into(),
                        cover: slint::Image::load_from_path(&thumb).unwrap_or_default(),
                        count: s.count as i32,
                    }
                }).collect();
                w.set_video_shows(slint::ModelRc::new(slint::VecModel::from(shows)));
                // "Next Up" rail (np.p3.episodes): one continue-watching tile
                // per show. tile.index maps into video_paths via video_ids so
                // the normal click→play path works unchanged.
                let id_pos: std::collections::HashMap<i64, i32> = video_ids().lock()
                    .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
                    .unwrap_or_default();
                let path_by_pos: Vec<String> = video_paths().lock()
                    .map(|g| g.iter().map(|p| p.to_string_lossy().into_owned()).collect())
                    .unwrap_or_default();
                let nu_tiles: Vec<VideoTile> = next_up.iter().filter_map(|n| {
                    let pos = *id_pos.get(&n.episode.item_id)?;
                    let abs = path_by_pos.get(pos as usize).cloned().unwrap_or_default();
                    let thumb = n.episode.still_path.as_deref()
                        .map(PathBuf::from).filter(|p| p.exists())
                        .or_else(|| by_path.get(&abs).cloned());
                    let dur_s = n.episode.runtime_min.unwrap_or(0) as f64 * 60.0;
                    let progress = match (n.resume_position_s, dur_s > 0.0) {
                        (Some(p), true) => (p / dur_s).clamp(0.0, 1.0) as f32,
                        _ => 0.0,
                    };
                    Some(VideoTile {
                        thumb: thumb.and_then(|t| slint::Image::load_from_path(&t).ok()).unwrap_or_default(),
                        label: format!("{} · S{:02}E{:02}{}", n.show_title, n.episode.season, n.episode.episode,
                            n.episode.title.as_deref().map(|t| format!(" — {t}")).unwrap_or_default()).into(),
                        index: pos,
                        starred: false, watched: false,
                        progress,
                        duration: n.episode.runtime_min.map(|m| format!("{m} min")).unwrap_or_default().into(),
                        season: n.episode.season as i32,
                        episode: n.episode.episode as i32,
                    })
                }).collect();
                w.set_video_next_up(slint::ModelRc::new(slint::VecModel::from(nu_tiles)));
                w.set_video_show_open(false);
                w.set_video_tiles(slint::ModelRc::new(slint::VecModel::from(Vec::<VideoTile>::new())));
                w.set_video_seasons(slint::ModelRc::new(slint::VecModel::from(Vec::<VideoSeason>::new())));
            });
        });
        return;
    }
    handle.spawn(async move {
        let rows = match pool_for("videos").await {
            Ok(pool) => video_rows_for(&pool, &category).await,
            Err(_) => Vec::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_show_open(video_show().lock().map(|g| g.is_some()).unwrap_or(false));
            // abs_path → (label, thumb_path) from the scan accumulator.
            let by_path: std::collections::HashMap<String, (String, PathBuf)> = video_full()
                .lock().map(|g| g.iter()
                    .map(|(label, orig, thumb)| (orig.to_string_lossy().into_owned(), (label.clone(), thumb.clone())))
                    .collect())
                .unwrap_or_default();
            let mut tiles: Vec<VideoTile> = Vec::with_capacity(rows.len());
            let mut paths: Vec<PathBuf> = Vec::with_capacity(rows.len());
            let mut ids: Vec<i64> = Vec::with_capacity(rows.len());
            for r in &rows {
                let (label, ffmpeg_thumb) = by_path.get(&r.abs_path).cloned().unwrap_or_else(|| {
                    let l = std::path::Path::new(&r.abs_path).file_name()
                        .and_then(|s| s.to_str()).unwrap_or("").to_string();
                    (l, PathBuf::from(&r.abs_path))
                });
                // Prefer TMDB poster (np.p3.tmdb) over ffmpeg frame thumbnail.
                let thumb = r.poster_local.as_deref()
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .unwrap_or(ffmpeg_thumb);
                let img = slint::Image::load_from_path(&thumb).unwrap_or_default();
                tiles.push(VideoTile {
                    thumb: img,
                    label: label.into(),
                    index: tiles.len() as i32,
                    starred: r.starred,
                    watched: r.watched,
                    progress: r.progress,
                    duration: r.duration.clone().into(),
                    season: r.season,
                    episode: r.episode,
                });
                paths.push(PathBuf::from(&r.abs_path));
                ids.push(r.item_id);
            }
            if let Ok(mut g) = video_paths().lock() { *g = paths; }
            if let Ok(mut g) = video_ids().lock() { *g = ids; }
            // Drilled into a TV show → group episodes by season for the browser
            // (np.p3.episodes). Rows are already ordered season,episode so a
            // single pass yields contiguous season groups; tiles keep their
            // global `index` so click→play still maps into video_paths.
            let drilled_tv = video_kind().lock().map(|g| *g == "tv").unwrap_or(false)
                && video_show().lock().map(|g| g.is_some()).unwrap_or(false);
            let seasons: Vec<VideoSeason> = if drilled_tv {
                let mut groups: Vec<(i32, Vec<VideoTile>)> = Vec::new();
                for t in &tiles {
                    if groups.last().map(|g| g.0) != Some(t.season) {
                        groups.push((t.season, Vec::new()));
                    }
                    groups.last_mut().unwrap().1.push(t.clone());
                }
                groups.into_iter().map(|(num, ts)| VideoSeason {
                    number: num,
                    label: if num > 0 { format!("Season {num}") } else { "Specials".to_string() }.into(),
                    tiles: slint::ModelRc::new(slint::VecModel::from(ts)),
                }).collect()
            } else {
                Vec::new()
            };
            w.set_video_seasons(slint::ModelRc::new(slint::VecModel::from(seasons)));
            w.set_video_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
        });
    });
}

// ── Books data layer (np.p4.books.library / progress) ───────────────────────
