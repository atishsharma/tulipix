//! Books/comics reader + library grid glue, extracted from tulipix-app::main.
//!
//! Owns the native reader (EPUB pagination, comic/PDF raster, continuous PDF
//! scroll), the library grid + continue-reading hero + detail sheet + series
//! strip, and every `window.on_book_*` callback (registered by [`wire_books`]).
//! Lower-level parsing/rasterisation lives in the sibling [`crate::books`] module.
#![allow(clippy::too_many_arguments)]

use slint::{ComponentHandle, Model};
use std::path::PathBuf;

use tulipix_common::{dirs_default, dirs_default_documents, pool_for};
use tulipix_core::proc::NoWindow;
use tulipix_ui::*;

use crate::books;

pub fn wire_books(window: &MainWindow) {
    // ── Books: library view tabs + reader (np.p4.books.*) ──
    let w = window.as_weak();
    window.on_book_set_view(move |v| {
        if let Some(w) = w.upgrade() {
            // Leaving a view clears any open series drill-in strip.
            w.set_book_series_strip(slint::ModelRc::new(slint::VecModel::from(Vec::<SeriesStripRow>::new())));
            w.set_book_series_strip_total(0);
            w.set_book_view(v.clone()); refresh_books(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_search(move |_q| {
        // `book-query` is bound in the .slint, so the window already holds the text.
        if let Some(w) = w.upgrade() { refresh_books(&w); }
    });
    let w = window.as_weak();
    window.on_book_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let mode = s.to_string();
        let cur_mode = w0.get_book_sort().to_string();
        let cur_dir = w0.get_book_sort_dir().to_string();
        // Re-click flips direction; switching mode resets (title/author = A→Z asc,
        // recent = newest first / desc).
        let dir = if mode == cur_mode {
            if cur_dir == "asc" { "desc" } else { "asc" }
        } else if mode == "recent" { "desc" } else { "asc" };
        w0.set_book_sort(mode.into());
        w0.set_book_sort_dir(dir.into());
        refresh_books(&w0);
    });
    let w = window.as_weak();
    window.on_book_open(move |idx| { open_book(w.clone(), idx); });
    let w = window.as_weak();
    window.on_book_reader_close(move || {
        if let Some(w) = w.upgrade() {
            save_reader_progress();
            w.set_book_reader_open(false);
        }
        reader_clear();
    });
    let w = window.as_weak();
    window.on_book_next(move || { if let Some(w) = w.upgrade() { reader_step(&w, 1); } });
    let w = window.as_weak();
    window.on_book_prev(move || { if let Some(w) = w.upgrade() { reader_step(&w, -1); } });
    let w = window.as_weak();
    window.on_book_scrub(move |frac| { if let Some(w) = w.upgrade() { reader_scrub(&w, frac); } });
    let w = window.as_weak();
    window.on_book_toggle_spread(move || { if let Some(w) = w.upgrade() { reader_toggle_spread(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_rtl(move || { if let Some(w) = w.upgrade() { reader_toggle_rtl(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_invert(move || { if let Some(w) = w.upgrade() { reader_toggle_invert(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_guided(move || {
        // np.p5.books.comic-guided — panel-by-panel reading on/off.
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    s.guided = !s.guided;
                    s.panel = 0;
                    s.panels_page = usize::MAX; // force re-detect on render
                }
            });
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_set_font(move |px| { if let Some(w) = w.upgrade() { reader_set_typo(&w, Some(px), None, None, None); } });
    let w = window.as_weak();
    window.on_book_set_line(move |lh| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, Some(lh), None, None); } });
    let w = window.as_weak();
    window.on_book_set_family(move |fam| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, None, Some(fam), None); } });
    let w = window.as_weak();
    window.on_book_set_margin(move |m| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, None, None, Some(m)); } });
    let w = window.as_weak();
    window.on_book_reflow(move |width, height| { if let Some(w) = w.upgrade() { reader_reflow(&w, width, height); } });
    let w = window.as_weak();
    window.on_book_toggle_text_dark(move || {
        if let Some(w) = w.upgrade() { w.set_book_text_dark(!w.get_book_text_dark()); }
    });

    // ── Books Phase 5: TOC / bookmarks / themes / series (np.p5.books.*) ──
    let w = window.as_weak();
    window.on_book_toggle_toc(move || {
        if let Some(w) = w.upgrade() { w.set_book_toc_open(!w.get_book_toc_open()); }
    });
    let w = window.as_weak();
    window.on_book_jump_chapter(move |page| {
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_text(&s.format) {
                        // `page` is a chapter index from the TOC → its first page.
                        let c = (page as usize).min(s.chapter_starts.len().saturating_sub(1));
                        s.page = s.chapter_starts.get(c).copied().unwrap_or(0);
                        s.chapter = c;
                    } else {
                        s.comic.page = (page as usize).min(s.comic.total.saturating_sub(1));
                        s.panel = 0; // guided view restarts on the jumped-to page
                    }
                }
            });
            reader_render(&w);
            save_reader_progress();
            w.set_book_toc_open(false);
        }
    });
    let w = window.as_weak();
    window.on_book_toggle_bookmarks(move || {
        if let Some(w) = w.upgrade() { w.set_book_bookmarks_open(!w.get_book_bookmarks_open()); }
    });
    let w = window.as_weak();
    window.on_book_add_bookmark(move || {
        let Some(w0) = w.upgrade() else { return; };
        let color = w0.get_book_bm_color().to_string();
        let snap = READER.with(|r| {
            r.borrow().as_ref().map(|s| {
                let page = if fmt_is_comic(&s.format) { s.comic.page as i64 } else { s.page as i64 };
                (s.item_id, page)
            })
        });
        let Some((item_id, page)) = snap else { return; };
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("books").await {
                let _ = tulipix_books::progress::add_bookmark(
                    &pool, item_id, &page.to_string(), page, None, Some(&color)).await;
                // Reload bookmarks into UI
                if let Ok(bms) = tulipix_books::progress::bookmarks(&pool, item_id).await {
                    let _ = wk.upgrade_in_event_loop(move |w| {
                        let rows: Vec<BookmarkRow> = bms.into_iter().map(|(id, pg, note, color)| BookmarkRow {
                            id: id as i32,
                            page: pg as i32,
                            note: note.unwrap_or_default().into(),
                            color: color.unwrap_or_default().into(),
                        }).collect();
                        w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(rows)));
                    });
                }
            }
        });
    });
    let w = window.as_weak();
    window.on_book_jump_bookmark(move |page| {
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_comic(&s.format) {
                        s.comic.page = (page as usize).min(s.comic.total.saturating_sub(1));
                        s.panel = 0;
                    } else {
                        s.page = (page as usize).min(s.pages.len().saturating_sub(1));
                        s.chapter = chapter_of_page(s, s.page);
                    }
                }
            });
            reader_render(&w);
            save_reader_progress();
            w.set_book_bookmarks_open(false);
        }
    });
    let w = window.as_weak();
    window.on_book_toggle_read_mode(move || {
        if let Some(w) = w.upgrade() {
            let new_mode = if w.get_book_read_mode().as_str() == "paginated" { "scroll" } else { "paginated" };
            w.set_book_read_mode(new_mode.into());
            // Scroll mode shows the whole chapter; paginated shows one page.
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_set_theme(move |t| {
        if let Some(w) = w.upgrade() { w.set_book_theme(t); }
    });
    let w = window.as_weak();
    window.on_book_tts_start(move || {
        // np.p4/p5.books.tts — read the current page aloud sentence by
        // sentence, surfacing the spoken sentence in the reader overlay.
        // Offline Piper voice first, platform TTS fallback per sentence.
        let Some(_w) = w.upgrade() else { return; };
        let text = READER.with(|r| r.borrow().as_ref()
            .and_then(|s| s.pages.get(s.page).cloned()).unwrap_or_default());
        if text.trim().is_empty() { return; }
        // A bumped generation stops any previous run; the stop button bumps too.
        let generation = TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let wk = w.clone();
        std::thread::spawn(move || {
            let sentences = tulipix_books::tts::chunk_sentences(&text, 400);
            for sentence in sentences {
                if TTS_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation { break; }
                let shown = sentence.clone();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    w.set_book_tts_sentence(shown.into());
                });
                tts_speak_sentence(&sentence);
            }
            // Clear the overlay only if no newer run took over.
            if TTS_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
                let _ = wk.upgrade_in_event_loop(move |w| {
                    w.set_book_tts_sentence("".into());
                });
            }
        });
    });
    let w = window.as_weak();
    window.on_book_tts_stop(move || {
        // Invalidate the running generation; the speaking thread notices
        // before its next sentence.
        TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(w) = w.upgrade() { w.set_book_tts_sentence("".into()); }
    });
    let w = window.as_weak();
    window.on_book_define(move |word| {
        // np.p5.books.dictionary — definition + translation + Wikipedia lookup.
        let Some(w0) = w.upgrade() else { return; };
        let word = word.to_string().trim().to_string();
        if word.is_empty() { return; }
        w0.set_book_define_result(format!("Looking up “{word}”…").into());
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let result = define_word(&word).await;
            let _ = wk.upgrade_in_event_loop(move |w| {
                w.set_book_define_result(result.into());
            });
        });
    });
    let w = window.as_weak();
    window.on_book_find_text(move |query| {
        if let Some(w) = w.upgrade() {
            let q = query.to_string().to_lowercase();
            if q.trim().is_empty() { return; }
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_text(&s.format) {
                        // Same query again = find-NEXT: scan after the current
                        // page and wrap; a new query starts from the top.
                        let total = s.pages.len();
                        let start = if s.last_query == q { s.page + 1 } else { 0 };
                        let hit = (0..total)
                            .map(|off| (start + off) % total.max(1))
                            .find(|&i| s.pages[i].to_lowercase().contains(&q));
                        if let Some(idx) = hit {
                            s.page = idx;
                            s.chapter = chapter_of_page(s, s.page);
                        }
                        s.last_query = q.clone();
                    }
                }
            });
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_series_open(move |series_id| {
        // Open the series drill-in strip: reading-order row with per-book status
        // and a "Next up" marker (np.p4.books.library). Switches to the All view
        // so the strip renders above the grid.
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let (sname, author): (String, String) = sqlx::query_as(
                "SELECT s.name,
                        COALESCE((SELECT bm.author FROM book_meta bm
                                  WHERE bm.series_id = s.id AND bm.author IS NOT NULL AND bm.author != '' LIMIT 1), '')
                 FROM series s WHERE s.id = ?")
                .bind(series_id as i64).fetch_optional(&pool).await.ok().flatten().unwrap_or_default();
            let rows: Vec<(String, i64, String, Option<String>, Option<i64>, i64, Option<i64>, i64, Option<f64>)> =
                sqlx::query_as(
                "SELECT i.abs_path, bm.item_id, COALESCE(bm.title,''), bm.cover_path, bm.page_count,
                        COALESCE(rp.page,0), rp.total_pages, COALESCE(rp.finished,0), bm.series_index
                 FROM book_meta bm
                 JOIN items i ON i.id = bm.item_id
                 LEFT JOIN reading_progress rp ON rp.item_id = bm.item_id
                 WHERE bm.series_id = ? AND i.missing_since IS NULL
                 ORDER BY bm.series_index, bm.title COLLATE NOCASE")
                .bind(series_id as i64).fetch_all(&pool).await.unwrap_or_default();
            let _ = wk.upgrade_in_event_loop(move |w| {
                let n = rows.len();
                let done = rows.iter().filter(|r| r.7 != 0).count();
                let mut strip: Vec<SeriesStripRow> = Vec::with_capacity(n);
                let mut spaths: Vec<(PathBuf, i64)> = Vec::with_capacity(n);
                let mut all_prev_finished = true;
                for (i, (abs_path, item_id, title, cover_path, page_count, page, total_pages, finished_i, series_index)) in rows.iter().enumerate() {
                    let finished = *finished_i != 0;
                    let total = total_pages.or(*page_count).unwrap_or(0);
                    let frac = if finished { 1.0 }
                               else if total > 0 { ((page + 1).min(total) as f32) / total as f32 } else { 0.0 };
                    let next_up = !finished && all_prev_finished;
                    if !finished { all_prev_finished = false; }
                    let status = if finished { "Finished".to_string() }
                        else if *page > 0 { format!("Reading · {}%", (frac * 100.0) as i32) }
                        else { "Unread".to_string() };
                    let num = series_index.map(|x| x as i32).unwrap_or((i + 1) as i32);
                    let has_cover = cover_path.is_some();
                    let img = cover_path.as_deref()
                        .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                        .unwrap_or_default();
                    strip.push(SeriesStripRow {
                        thumb: img, has_cover, tint: paper_tint(title),
                        title: title.clone().into(), status: status.into(), num,
                        next_up, finished, frac, index: i as i32,
                    });
                    spaths.push((PathBuf::from(abs_path), *item_id));
                }
                if let Ok(mut g) = book_strip().lock() { *g = spaths; }
                w.set_book_series_strip_title(sname.clone().into());
                w.set_book_series_strip_sub(format!("{author} · {n} books · publication order").into());
                w.set_book_series_strip_done(done as i32);
                w.set_book_series_strip_total(n as i32);
                w.set_book_series_strip(slint::ModelRc::new(slint::VecModel::from(strip)));
                w.set_book_view("all".into());
                refresh_books(&w);
            });
        });
    });
    let w = window.as_weak();
    window.on_book_collection_open(move |col_id| {
        if let Some(w) = w.upgrade() {
            w.set_book_view(format!("col:{col_id}").into());
            refresh_books(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_add_to_collection(move |idx, name| {
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let name = name.to_string().trim().to_string();
        if item_id < 0 || name.is_empty() { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let _ = tulipix_books::library::add_to_collection(&pool, &name, item_id).await;
            let _ = wk.upgrade_in_event_loop(move |w| refresh_books(&w));
        });
    });
    let w = window.as_weak();
    window.on_book_fetch_meta(move |idx| {
        // np.p4.books.metadata — Google Books title lookup; fills only empty
        // book_meta fields (metadata::apply COALESCEs), then refreshes the grid.
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let stem = book_paths().lock().ok()
            .and_then(|g| g.get(idx as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str().map(|s| s.to_string())))
            .unwrap_or_default();
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            // Prefer the stored title; fall back to the file stem.
            let title: String = sqlx::query_scalar::<_, Option<String>>(
                "SELECT title FROM book_meta WHERE item_id = ?1")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(stem);
            if title.trim().is_empty() { return; }
            let url = tulipix_books::metadata::google_books_url(&title);
            let Ok(resp) = reqwest::get(&url).await else { return; };
            let Ok(body) = resp.text().await else { return; };
            let Some(meta) = tulipix_books::metadata::parse_google_books(&body) else { return; };
            let _ = tulipix_books::metadata::apply(&pool, item_id, &meta).await;
            let _ = wk.upgrade_in_event_loop(move |w| refresh_books(&w));
        });
    });
    let w = window.as_weak();
    window.on_book_export_notes(move |idx| {
        // np.p5.books.export-notes — Markdown + CSV into ~/Documents/Tulipix
        // Notes/, with a visible confirmation in the books stats bar.
        let Some(_w0) = w.upgrade() else { return; };
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let title = book_paths().lock().ok()
            .and_then(|g| g.get(idx as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str().map(|s| s.to_string())))
            .unwrap_or_default();
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let status = async {
                let pool = pool_for("books").await.ok()?;
                let bms = tulipix_books::progress::bookmarks(&pool, item_id).await.ok()?;
                if bms.is_empty() {
                    return Some("No bookmarks to export for this book".to_string());
                }
                let dir = dirs_default_documents().join("Tulipix Notes");
                std::fs::create_dir_all(&dir).ok()?;
                let safe: String = title.chars()
                    .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
                    .collect();
                let mut md = format!("# Bookmarks: {title}\n\n");
                let mut csv = String::from("page,color,note\n");
                for (_id, page, note, color) in &bms {
                    let n = note.as_deref().unwrap_or("");
                    let c = color.as_deref().unwrap_or("");
                    md.push_str(&format!("- **Page {}**{}{}\n", page + 1,
                        if c.is_empty() { String::new() } else { format!(" `{c}`") },
                        if n.is_empty() { String::new() } else { format!(" — {n}") }));
                    csv.push_str(&format!("{},{},\"{}\"\n", page + 1, c, n.replace('"', "\"\"")));
                }
                std::fs::write(dir.join(format!("{safe}.md")), &md).ok()?;
                std::fs::write(dir.join(format!("{safe}.csv")), &csv).ok()?;
                Some(format!("{} notes exported → {}", bms.len(), dir.display()))
            }.await.unwrap_or_else(|| "Export failed — see logs".to_string());
            let _ = wk.upgrade_in_event_loop(move |w| {
                w.set_book_export_status(status.into());
                // Auto-clear after 6s without holding the window alive.
                let weak = w.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_secs(6), move || {
                    if let Some(w) = weak.upgrade() { w.set_book_export_status("".into()); }
                });
            });
        });
    });
    let w = window.as_weak();
    window.on_book_goal_adjust(move |delta| {
        // ± yearly reading goal (np.p5.books.stats); persisted in book_prefs.
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let cur = tulipix_books::progress::get_pref(&pool, "year_goal").await
                .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(12);
            let next = (cur + delta as i64).clamp(1, 999);
            let _ = tulipix_books::progress::set_pref(&pool, "year_goal", &next.to_string()).await;
            let _ = wk.upgrade_in_event_loop(move |w| { refresh_books(&w); });
        });
    });

    // Resume a continue-reading hero card (hero-list index → book).
    let w = window.as_weak();
    window.on_book_resume(move |i| {
        if let Some((path, item_id)) = book_hero().lock().ok().and_then(|g| g.get(i as usize).cloned()) {
            open_book_path(w.clone(), path, item_id);
        }
    });
    // Hero "Details" → resolve to a grid index and open the detail sheet.
    let w = window.as_weak();
    window.on_book_hero_detail(move |i| {
        let Some((path, item_id)) = book_hero().lock().ok().and_then(|g| g.get(i as usize).cloned()) else { return; };
        let grid_idx = book_ids().lock().ok()
            .and_then(|g| g.iter().position(|&id| id == item_id)).map(|p| p as i32).unwrap_or(-1);
        kick_book_detail(w.clone(), grid_idx, item_id, path);
    });
    // Tile click → detail sheet (grid index).
    let w = window.as_weak();
    window.on_book_open_detail(move |idx| {
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let path = book_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        if let (true, Some(path)) = (item_id >= 0, path) { kick_book_detail(w.clone(), idx, item_id, path); }
    });
    // Mark a book finished from the detail sheet.
    let w = window.as_weak();
    window.on_book_mark_finished(move |idx| {
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        if item_id < 0 { return; }
        let wk = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let _ = sqlx::query(
                "INSERT INTO reading_progress (item_id, page, finished, updated) VALUES (?, 0, 1, strftime('%s','now'))
                 ON CONFLICT(item_id) DO UPDATE SET finished = 1, updated = strftime('%s','now')")
                .bind(item_id).execute(&pool).await;
            let _ = wk.upgrade_in_event_loop(|w| refresh_books(&w));
        });
    });
    // Remove a book from the library (DB only; file stays on disk).
    let w = window.as_weak();
    window.on_book_remove(move |idx| {
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        if item_id < 0 { return; }
        let wk = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let _ = sqlx::query("DELETE FROM items WHERE id = ?").bind(item_id).execute(&pool).await;
            let _ = wk.upgrade_in_event_loop(|w| refresh_books(&w));
        });
    });
    // Open a book from the series drill-in strip (strip-list index → book).
    let w = window.as_weak();
    window.on_book_series_strip_open(move |i| {
        if let Some((path, item_id)) = book_strip().lock().ok().and_then(|g| g.get(i as usize).cloned()) {
            open_book_path(w.clone(), path, item_id);
        }
    });
    // PDF reader — lazily rasterise a page as it scrolls into view, then drop it
    // into the pages image model (np.p5.books.pdf).
    let w = window.as_weak();
    window.on_book_pdf_page_visible(move |idx| {
        let path = READER.with(|r| r.borrow().as_ref().map(|s| s.path.clone()));
        let Some(path) = path else { return; };
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn_blocking(move || {
            let rendered = books::pdf_raster_page_image(&path, idx as usize);
            let _ = wk.upgrade_in_event_loop(move |w| {
                let Some(p) = rendered else { return; };
                let Ok(img) = slint::Image::load_from_path(&p) else { return; };
                let model = w.get_book_pdf_pages();
                if let Some(vm) = model.as_any().downcast_ref::<slint::VecModel<slint::Image>>() {
                    if (idx as usize) < vm.row_count() { vm.set_row_data(idx as usize, img); }
                }
            });
        });
    });
    // PDF reader — jump to a page (thumb click / scroll), updating progress.
    let w = window.as_weak();
    window.on_book_pdf_goto_page(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        READER.with(|r| { if let Some(s) = r.borrow_mut().as_mut() {
            s.comic.page = (idx as usize).min(s.comic.total.saturating_sub(1));
        }});
        reader_render(&w0);
    });
}
// Video library rows/discover/refresh extracted to tulipix_sec_videos.
// book_paths[i] / book_ids[i] map a library-grid tile index → its file + item.
static BOOK_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
fn book_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    BOOK_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
static BOOK_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn book_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    BOOK_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Continue-reading hero list + opened-series strip list: (path, item_id) in the
// order shown, so resume/hero-detail/strip-open resolve their index → book.
static BOOK_HERO: std::sync::OnceLock<std::sync::Mutex<Vec<(PathBuf, i64)>>> = std::sync::OnceLock::new();
fn book_hero() -> &'static std::sync::Mutex<Vec<(PathBuf, i64)>> {
    BOOK_HERO.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
static BOOK_STRIP: std::sync::OnceLock<std::sync::Mutex<Vec<(PathBuf, i64)>>> = std::sync::OnceLock::new();
fn book_strip() -> &'static std::sync::Mutex<Vec<(PathBuf, i64)>> {
    BOOK_STRIP.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
struct BookRow {
    abs_path: String,
    item_id: i64,
    title: String,
    author: String,
    cover_path: Option<String>,
    is_comic: bool,
    #[allow(dead_code)] // reading direction; carried for future right-to-left comic support
    rtl: bool,
    format: String,
    page: i64,
    total: Option<i64>,
    finished: bool,
}
/// Deterministic warm-paper tint for a generated cover — lerps between two
/// paper anchors by a hash of the title, keeping saturation low (paper, not
/// candy). The Slint side darkens this for dark mode.
fn paper_tint(title: &str) -> slint::Color {
    let mut h: u32 = 2166136261;
    for b in title.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619); }
    let t = (h % 1000) as f32 / 1000.0;
    // #ece3cf → #d8c8a8 (light, subtle warm range).
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    slint::Color::from_rgb_u8(lerp(236, 216), lerp(227, 200), lerp(207, 168))
}
/// Query book_meta + reading_progress for the present library, filtered by the
/// active view tab (all / reading / unread / comics).
async fn book_rows_for(pool: &sqlx::SqlitePool, view: &str, query: &str, sort: &str, dir: &str) -> Vec<BookRow> {
    // A drilled-in shelf is "col:<id>" — the id is parsed (never interpolated raw).
    let col_id = view.strip_prefix("col:").and_then(|v| v.parse::<i64>().ok());
    let col_filter = col_id.map(|id| {
        format!("AND bm.item_id IN (SELECT item_id FROM collection_items WHERE collection_id = {id})")
    });
    let filter = match view {
        "reading" => "AND COALESCE(rp.page,0) > 0 AND COALESCE(rp.finished,0) = 0",
        "unread"  => "AND (rp.item_id IS NULL OR (COALESCE(rp.page,0) = 0 AND COALESCE(rp.finished,0) = 0))",
        "comics"  => "AND bm.is_comic = 1",
        _          => col_filter.as_deref().unwrap_or(""),
    };
    let dir_sql = if dir.eq_ignore_ascii_case("desc") { "DESC" } else { "ASC" };
    let order = match sort {
        "author" => format!("bm.author COLLATE NOCASE {dir_sql}"),
        "recent" => format!("COALESCE(rp.updated,0) {dir_sql}, bm.title COLLATE NOCASE"),
        _         => format!("bm.title COLLATE NOCASE {dir_sql}"),
    };
    // Free-text filter over title + author (bound, so no injection).
    let q = query.trim();
    let search = if q.is_empty() { "" } else { "AND (bm.title LIKE ?1 OR bm.author LIKE ?1)" };
    let sql = format!(
        "SELECT i.abs_path, bm.item_id, COALESCE(bm.title,''), COALESCE(bm.author,''), bm.cover_path,
                bm.is_comic, bm.rtl, bm.format,
                COALESCE(rp.page,0), rp.total_pages, COALESCE(rp.finished,0)
         FROM book_meta bm JOIN items i ON i.id = bm.item_id
         LEFT JOIN reading_progress rp ON rp.item_id = bm.item_id
         WHERE i.missing_since IS NULL {filter} {search}
         ORDER BY {order}",
    );
    let mut qb = sqlx::query_as(&sql);
    if !q.is_empty() { qb = qb.bind(format!("%{q}%")); }
    let rows: Vec<(String, i64, String, String, Option<String>, i64, i64, String, i64, Option<i64>, i64)> =
        qb.fetch_all(pool).await.unwrap_or_default();
    rows.into_iter().map(|(abs_path, item_id, title, author, cover_path, is_comic, rtl, format, page, total, finished)| {
        BookRow { abs_path, item_id, title, author, cover_path, is_comic: is_comic != 0, rtl: rtl != 0, format, page, total, finished: finished != 0 }
    }).collect()
}
/// Re-query the Books grid from the window's current view/query/sort state.
pub fn refresh_books(w: &MainWindow) {
    kick_books_refresh(
        w.as_weak(),
        w.get_book_view().to_string(),
        w.get_book_query().to_string(),
        w.get_book_sort().to_string(),
        w.get_book_sort_dir().to_string(),
    );
}
/// Build the detail-sheet payload for a book and open the sheet. `grid_idx` is
/// the tile index the sheet's actions (open-book / mark-finished / remove /
/// shelf / fetch-meta) operate on.
fn kick_book_detail(weak: slint::Weak<MainWindow>, grid_idx: i32, item_id: i64, path: PathBuf) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("books").await else { return; };
        let meta: Option<(String, String, String, Option<String>, Option<i64>, Option<String>)> =
            sqlx::query_as(
            "SELECT COALESCE(title,''), COALESCE(author,''), format, description, page_count, cover_path
             FROM book_meta WHERE item_id = ?").bind(item_id).fetch_optional(&pool).await.ok().flatten();
        let prog: Option<(i64, Option<i64>, i64)> = sqlx::query_as(
            "SELECT COALESCE(page,0), total_pages, COALESCE(finished,0) FROM reading_progress WHERE item_id = ?")
            .bind(item_id).fetch_optional(&pool).await.ok().flatten();
        let shelf: Option<String> = sqlx::query_scalar(
            "SELECT c.name FROM collections c JOIN collection_items ci ON ci.collection_id = c.id
             WHERE ci.item_id = ? LIMIT 1").bind(item_id).fetch_optional(&pool).await.ok().flatten();
        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let _ = weak.upgrade_in_event_loop(move |w| {
            let (title, author, format, desc, page_count, cover_path) = meta.unwrap_or_default();
            let (page, total_pages, finished_i) = prog.unwrap_or((0, None, 0));
            let finished = finished_i != 0;
            let total = total_pages.or(page_count).unwrap_or(0);
            let has_progress = page > 0 && !finished && total > 0;
            let frac = if finished { 1.0 }
                       else if total > 0 { ((page + 1).min(total) as f32) / total as f32 } else { 0.0 };
            let pages_left = (total - (page + 1)).max(0);
            let secs_left = pages_left * 90;
            let left_label = if has_progress {
                let (h, m) = (secs_left / 3600, (secs_left % 3600) / 60);
                if h > 0 { format!("≈ {h}h {m}m left") } else { format!("≈ {m}m left") }
            } else { String::new() };
            let page_label = if total > 0 { format!("Page {} / {}", page + 1, total) } else { String::new() };
            let eyebrow = format!("{}{}", format.to_uppercase(),
                if finished { " · Finished" } else if has_progress { " · In progress" } else { " · Unread" });
            let size = if size_bytes >= 1_048_576 { format!("{:.1} MB", size_bytes as f64 / 1_048_576.0) }
                       else if size_bytes >= 1024 { format!("{} KB", size_bytes / 1024) }
                       else { format!("{size_bytes} B") };
            let pages = if total > 0 { format!("{total} pages") } else { "— pages".to_string() };
            let shelf_s = shelf.map(|s| format!("Shelf: {s}")).unwrap_or_else(|| "Shelf: —".to_string());
            let has_cover = cover_path.is_some();
            let img = cover_path.as_deref()
                .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                .unwrap_or_default();
            let desc = desc.filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| "No description yet — Fetch metadata pulls the synopsis and page count.".to_string());
            let tint = paper_tint(&title);
            w.set_book_detail(BookDetail {
                title: title.into(), author: author.into(), eyebrow: eyebrow.into(),
                desc: desc.into(), format: format.to_uppercase().into(), size: size.into(),
                pages: pages.into(), shelf: shelf_s.into(), frac,
                page_label: page_label.into(), left_label: left_label.into(),
                has_progress, finished, thumb: img, has_cover, tint, index: grid_idx,
            });
            w.set_book_detail_idx(0);
        });
    });
}
fn kick_books_refresh(weak: slint::Weak<MainWindow>, view: String, query: String, sort: String, dir: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (rows, series_data, author_data, col_data, read_stats, hero_data) = match pool_for("books").await {
            Ok(pool) => {
                let r = book_rows_for(&pool, &view, &query, &sort, &dir).await;
                // Reading time / streak / yearly goal (np.p5.books.stats).
                let rs = tulipix_books::progress::reading_stats(&pool).await.unwrap_or_default();
                // Load series: id, name, count, cover_path
                let s: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
                    "SELECT s.id, s.name, COUNT(bm.item_id), MIN(bm.cover_path)
                     FROM series s JOIN book_meta bm ON bm.series_id = s.id
                     JOIN items i ON i.id = bm.item_id
                     WHERE i.missing_since IS NULL
                     GROUP BY s.id ORDER BY s.name COLLATE NOCASE"
                ).fetch_all(&pool).await.unwrap_or_default();
                // Author pages: name + book count (np.p4.books.library).
                let a = tulipix_books::library::authors(&pool).await.unwrap_or_default();
                // Shelves (collections): id, name, count, a member cover.
                let c: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
                    "SELECT c.id, c.name, COUNT(ci.item_id),
                            (SELECT bm.cover_path FROM collection_items ci2
                             JOIN book_meta bm ON bm.item_id = ci2.item_id
                             WHERE ci2.collection_id = c.id AND bm.cover_path IS NOT NULL
                             LIMIT 1)
                     FROM collections c LEFT JOIN collection_items ci ON ci.collection_id = c.id
                     GROUP BY c.id ORDER BY c.name COLLATE NOCASE"
                ).fetch_all(&pool).await.unwrap_or_default();
                // Continue-reading hero: most-recent unfinished books.
                let hero: Vec<(String, i64, String, String, Option<String>, String, i64, Option<i64>, Option<i64>, Option<String>, Option<f64>)> =
                    sqlx::query_as(
                    "SELECT i.abs_path, bm.item_id, COALESCE(bm.title,''), COALESCE(bm.author,''),
                            bm.cover_path, bm.format, COALESCE(rp.page,0), rp.total_pages, bm.page_count,
                            (SELECT s.name FROM series s WHERE s.id = bm.series_id), bm.series_index
                     FROM reading_progress rp
                     JOIN book_meta bm ON bm.item_id = rp.item_id
                     JOIN items i ON i.id = bm.item_id
                     WHERE i.missing_since IS NULL AND COALESCE(rp.finished,0) = 0 AND COALESCE(rp.page,0) > 0
                     ORDER BY rp.updated DESC LIMIT 3"
                ).fetch_all(&pool).await.unwrap_or_default();
                (r, s, a, c, rs, hero)
            },
            Err(_) => (Vec::new(), Vec::new(), Vec::new(), Vec::new(),
                       tulipix_books::progress::ReadingStats::default(), Vec::new()),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let mut tiles: Vec<BookTile> = Vec::with_capacity(rows.len());
            let mut paths: Vec<PathBuf> = Vec::with_capacity(rows.len());
            let mut ids: Vec<i64> = Vec::with_capacity(rows.len());
            for r in &rows {
                // Cover: extracted EPUB/CBZ cover if we have one, else fall back
                // to the scan thumb path for the file (placeholder for PDF/CBR).
                let cover = r.cover_path.clone().unwrap_or_else(|| r.abs_path.clone());
                let img = slint::Image::load_from_path(std::path::Path::new(&cover)).unwrap_or_default();
                let progress = match r.total {
                    Some(t) if t > 0 => ((r.page + 1).min(t) as f32) / t as f32,
                    _ => if r.finished { 1.0 } else { 0.0 },
                };
                tiles.push(BookTile {
                    thumb: img,
                    title: r.title.clone().into(),
                    author: r.author.clone().into(),
                    index: tiles.len() as i32,
                    progress,
                    finished: r.finished,
                    comic: r.is_comic,
                    format: r.format.clone().into(),
                    has_cover: r.cover_path.is_some(),
                    tint: paper_tint(&r.title),
                });
                paths.push(PathBuf::from(&r.abs_path));
                ids.push(r.item_id);
            }
            if let Ok(mut g) = book_paths().lock() { *g = paths; }
            if let Ok(mut g) = book_ids().lock() { *g = ids; }
            // Series tiles
            let series_tiles: Vec<SeriesRow> = series_data.into_iter().map(|(id, name, count, cover)| {
                let img = cover.as_deref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                    .unwrap_or_default();
                SeriesRow { id: id as i32, name: name.into(), book_count: count as i32, cover: img }
            }).collect();
            w.set_book_series(slint::ModelRc::new(slint::VecModel::from(series_tiles)));
            // Author cards (np.p4.books.library — author pages)
            let author_rows: Vec<AuthorRow> = author_data.into_iter().map(|(name, count)| AuthorRow {
                name: name.into(), book_count: count as i32,
            }).collect();
            w.set_book_authors(slint::ModelRc::new(slint::VecModel::from(author_rows)));
            // Shelf cards (collections)
            let col_rows: Vec<CollectionRow> = col_data.into_iter().map(|(id, name, count, cover)| {
                let img = cover.as_deref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                    .unwrap_or_default();
                CollectionRow { id: id as i32, name: name.into(), book_count: count as i32, cover: img }
            }).collect();
            w.set_book_collections(slint::ModelRc::new(slint::VecModel::from(col_rows)));
            // Structured stat cards (np.p5.books.stats). The old single-string
            // `book-stats` strip was replaced by chip cards; only the in-progress
            // count is still derived from the rows.
            let reading = rows.iter().filter(|r| !r.finished && r.page > 0).count();
            let (sh, sm) = (read_stats.total_seconds / 3600, (read_stats.total_seconds % 3600) / 60);
            let stat_time = if read_stats.total_seconds < 60 { String::new() }
                else if sh > 0 { format!("{sh}h {sm}m") } else { format!("{sm}m") };
            w.set_book_goal_done(read_stats.finished_this_year as i32);
            w.set_book_goal_total(read_stats.year_goal.max(1) as i32);
            w.set_book_stat_time(stat_time.into());
            w.set_book_streak_days(read_stats.streak_days as i32);
            w.set_book_in_progress(reading as i32);
            // Continue-reading hero rows (np.p5.books).
            let mut hero_rows: Vec<HeroRow> = Vec::new();
            let mut hero_paths: Vec<(PathBuf, i64)> = Vec::new();
            for (abs_path, item_id, title, author, cover_path, _format, page, total_pages, page_count, series_name, series_index) in &hero_data {
                let total = total_pages.or(*page_count).unwrap_or(0);
                let frac = if total > 0 { ((page + 1).min(total) as f32) / total as f32 } else { 0.0 };
                let pages_left = (total - (page + 1)).max(0);
                let secs_left = pages_left * 90;
                let left_label = if total > 0 {
                    let (h, m) = (secs_left / 3600, (secs_left % 3600) / 60);
                    if h > 0 { format!("≈ {h}h {m}m left") } else { format!("≈ {m}m left") }
                } else { String::new() };
                let page_label = if total > 0 {
                    format!("Page {} / {} · {}%", page + 1, total, (frac * 100.0) as i32)
                } else { String::new() };
                let author_line = match (series_name, series_index) {
                    (Some(s), Some(ix)) => format!("{author} · {s} #{}", *ix as i64),
                    (Some(s), None)     => format!("{author} · {s}"),
                    _ => author.clone(),
                };
                let has_cover = cover_path.is_some();
                let img = cover_path.as_deref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                    .unwrap_or_default();
                hero_rows.push(HeroRow {
                    thumb: img, has_cover, tint: paper_tint(title),
                    title: title.clone().into(), author: author_line.into(),
                    frac, page_label: page_label.into(), left_label: left_label.into(),
                    index: hero_rows.len() as i32,
                });
                hero_paths.push((PathBuf::from(abs_path), *item_id));
            }
            if let Ok(mut g) = book_hero().lock() { *g = hero_paths; }
            w.set_book_hero_rows(slint::ModelRc::new(slint::VecModel::from(hero_rows)));
            w.set_book_count(tiles.len() as i32);
            w.set_book_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
        });
    });
}
// ── Books reader (np.p4.books.reader / .navigation / .typography / .progress) ─
// The open book lives on the UI thread; comics page via ReaderState, EPUBs via
// a chapter list, both honouring saved reading_progress.
struct ReaderSession {
    item_id: i64,
    path: PathBuf,
    format: String,
    comic: tulipix_books::reader::ReaderState, // page/total/spread/rtl/invert (comics)
    chapters: Vec<String>,                     // EPUB plain-text per spine entry
    chapter: usize,
    typo: tulipix_books::typography::Typography,
    // EPUB pagination: `chapters` flattened into screen-pages for the given
    // geometry + typography. `chapter_starts[c]` is the flat page index where
    // chapter `c` begins (TOC jump + chapter label).
    pages: Vec<String>,
    chapter_starts: Vec<usize>,
    page: usize,    // current flat screen-page (text mode)
    page_w: f32,    // text-column px (for re-pagination)
    page_h: f32,
    turn: i32,      // bumped on each page turn → drives the fold animation
    // Comic guided view (np.p5.books.comic-guided): panel-by-panel stepping.
    guided: bool,
    panel: usize,                        // current panel on the current page
    panels: Vec<(u32, u32, u32, u32)>,   // detected rects for `panels_page`
    panels_page: usize,                  // page the cache belongs to
    // Find-in-book (np.p5.books.fulltext): repeating the query finds the NEXT
    // match after the current page (wraps).
    last_query: String,
    // Last rendered page faces — become the flip overlay's outgoing pages.
    last_left: String,
    last_right: String,
    // Reading-time tracking (np.p5.books.stats): start of the unflushed slice;
    // save_reader_progress flushes elapsed into reading_sessions and resets.
    read_since: std::time::Instant,
}
thread_local! {
    static READER: std::cell::RefCell<Option<ReaderSession>> = const { std::cell::RefCell::new(None) };
}
fn reader_clear() { READER.with(|r| *r.borrow_mut() = None); }

/// Reader-side format families (np.p5.books.formats): comics + raster
/// documents (scanned PDF via poppler, DjVu via djvulibre) page through
/// images, everything else reflows as paginated text.
fn fmt_is_comic(f: &str) -> bool { matches!(f, "cbz" | "cbr" | "pdf-raster" | "djvu") }
fn fmt_is_text(f: &str) -> bool { matches!(f, "epub" | "mobi" | "azw3" | "fb2" | "pdf") }
/// Read-aloud run counter: bumping it cancels the active sentence loop
/// (np.p5.books.tts). Monotonic; each start claims the new value.
static TTS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Speak one sentence, blocking until audio finishes: Piper first, then the
/// platform voice. Sentence-sized calls keep the stop button responsive.
fn tts_speak_sentence(text: &str) {
    if tts_speak_piper(text) { return; }
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("spd-say").arg("--wait").arg(text).status()
        .or_else(|_| std::process::Command::new("espeak").arg(text).status());
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("say").arg(text).status();
    #[cfg(target_os = "windows")]
    let _ = {
        let ps = format!("Add-Type -AssemblyName System.Speech; \
            (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak(@'\n{}\n'@)",
            text.replace('\'', "''"));
        std::process::Command::new("powershell").args(["-NoProfile","-Command",&ps]).no_window().status()
    };
}
/// np.p4.books.tts — synthesize `text` with Piper (bundled or PATH binary +
/// the first .onnx voice in <data>/models/piper/) and play the wav. False when
/// any piece is missing so the caller can fall back to the platform voice.
fn tts_speak_piper(text: &str) -> bool {
    let attempt = || -> Option<()> {
        let piper = tulipix_core::thumbs::tool_bin("piper");
        let voices = dirs_default()?.join("models").join("piper");
        let voice = std::fs::read_dir(&voices).ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().map(|e| e == "onnx").unwrap_or(false))?;
        let wav = std::env::temp_dir().join("tulipix-tts.wav");
        let mut child = std::process::Command::new(&piper)
            .arg("--model").arg(&voice)
            .arg("--output_file").arg(&wav)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .no_window()
            .spawn().ok()?;
        {
            use std::io::Write;
            child.stdin.take()?.write_all(text.as_bytes()).ok()?;
        }
        if !child.wait().ok()?.success() { return None; }
        // Play the wav with whatever audio CLI is around (bundled ffplay first).
        let ffplay = tulipix_core::thumbs::tool_bin("ffplay");
        let players: [(String, Vec<&str>); 4] = [
            (ffplay.display().to_string(), vec!["-nodisp", "-autoexit", "-loglevel", "quiet"]),
            (tulipix_core::thumbs::tool_bin("mpv").display().to_string(), vec!["--no-video", "--really-quiet"]),
            ("paplay".into(), vec![]),
            ("aplay".into(), vec!["-q"]),
        ];
        players.iter().any(|(bin, args)| {
            std::process::Command::new(bin).args(args).arg(&wav)
                .no_window().status().map(|s| s.success()).unwrap_or(false)
        }).then_some(())
    };
    attempt().is_some()
}
/// np.p5.books.dictionary — look a word up: dictionaryapi.dev definition,
/// MyMemory translation into the system locale, Wikipedia summary. Each source
/// is best-effort; whatever answered is concatenated.
async fn define_word(word: &str) -> String {
    let q: String = word.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '\'' { c.to_string() }
             else { format!("%{:02X}", c as u32) })
        .collect();
    let mut out = String::new();
    // Dictionary definitions.
    if let Ok(r) = reqwest::get(format!("https://api.dictionaryapi.dev/api/v2/entries/en/{q}")).await {
        if let Ok(j) = r.json::<serde_json::Value>().await {
            if let Some(meanings) = j.get(0).and_then(|e| e.get("meanings")).and_then(|m| m.as_array()) {
                for m in meanings.iter().take(3) {
                    let pos = m.get("partOfSpeech").and_then(|p| p.as_str()).unwrap_or("");
                    if let Some(def) = m.pointer("/definitions/0/definition").and_then(|d| d.as_str()) {
                        out.push_str(&format!("• ({pos}) {def}\n"));
                    }
                }
            }
        }
    }
    // Translation into the system locale (skipped when the locale is English).
    let lang = std::env::var("LANG").unwrap_or_default()
        .get(0..2).unwrap_or("en").to_string();
    if lang != "en" && !lang.is_empty() {
        if let Ok(r) = reqwest::get(format!(
            "https://api.mymemory.translated.net/get?q={q}&langpair=en|{lang}")).await {
            if let Ok(j) = r.json::<serde_json::Value>().await {
                if let Some(t) = j.pointer("/responseData/translatedText").and_then(|t| t.as_str()) {
                    if !t.trim().is_empty() {
                        out.push_str(&format!("\n🌐 {lang}: {t}\n"));
                    }
                }
            }
        }
    }
    // Wikipedia summary.
    if let Ok(r) = reqwest::get(format!("https://en.wikipedia.org/api/rest_v1/page/summary/{q}")).await {
        if let Ok(j) = r.json::<serde_json::Value>().await {
            if let Some(extract) = j.get("extract").and_then(|e| e.as_str()) {
                if !extract.trim().is_empty() {
                    out.push_str(&format!("\n📖 Wikipedia: {extract}\n"));
                }
            }
        }
    }
    if out.trim().is_empty() {
        format!("No results for “{word}”. Check the spelling or try a simpler form.")
    } else {
        out
    }
}
fn family_to_int(f: tulipix_books::typography::FontFamily) -> i32 {
    use tulipix_books::typography::FontFamily::*;
    match f { Serif => 0, SansSerif => 1, OpenDyslexic => 2 }
}
fn int_to_family(i: i32) -> tulipix_books::typography::FontFamily {
    use tulipix_books::typography::FontFamily::*;
    match i { 1 => SansSerif, 2 => OpenDyslexic, _ => Serif }
}
/// Re-paginate the EPUB chapters into screen-pages for the session's current
/// geometry + typography. `preserve` keeps the reader near the same spot (by
/// fraction) across a reflow (font/margin/resize change).
fn reader_repaginate(s: &mut ReaderSession, preserve: bool) {
    use tulipix_books::paginate;
    let cap = paginate::chars_per_page(
        s.page_w, s.page_h, s.typo.font_px as f32, s.typo.line_height as f32, s.typo.family);
    let old_page = s.page;
    let old_total = s.pages.len();
    let mut pages: Vec<String> = Vec::new();
    let mut starts: Vec<usize> = Vec::with_capacity(s.chapters.len());
    for ch in &s.chapters {
        starts.push(pages.len());
        pages.extend(paginate::paginate(ch, cap));
    }
    if pages.is_empty() { pages.push(String::new()); starts = vec![0]; }
    s.chapter_starts = starts;
    s.pages = pages;
    s.page = if preserve {
        paginate::reflow_anchor(old_page, old_total, s.pages.len())
    } else {
        s.page.min(s.pages.len() - 1)
    };
    s.chapter = chapter_of_page(s, s.page);
}
/// Which chapter a flat screen-page belongs to.
fn chapter_of_page(s: &ReaderSession, page: usize) -> usize {
    match s.chapter_starts.binary_search(&page) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}
fn open_book(weak: slint::Weak<MainWindow>, idx: i32) {
    let Some(path) = book_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else { return; };
    let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
    open_book_path(weak, path, item_id);
}
// Open a specific book (by absolute path + item id) in the reader — shared by the
// Books grid and the Home Books card, which each resolve their own path/id.
pub fn open_book_path(weak: slint::Weak<MainWindow>, path: PathBuf, item_id: i64) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Saved progress (page index).
        let saved = match pool_for("books").await {
            Ok(pool) => tulipix_books::progress::get(&pool, item_id).await.ok().flatten(),
            Err(_) => None,
        };
        let saved_page = saved.map(|(p, _, _)| p as usize).unwrap_or(0);
        let pb = path.clone();
        // Heavy extraction off the UI thread.
        let prep = tokio::task::spawn_blocking(move || {
            let ingest = books::ingest_file(&pb);
            let fmt = ingest.as_ref().map(|i| i.format).unwrap_or("");
            let rtl = ingest.as_ref().map(|i| i.rtl).unwrap_or(false);
            match fmt {
                "cbz" | "cbr" | "djvu" => {
                    let total = books::comic_page_count(&pb);
                    // 0 pages = missing system tool (unrar/ddjvu) or bad file.
                    if total == 0 {
                        (format!("{fmt}-empty"), 0, Vec::new(), false)
                    } else {
                        (fmt.to_string(), total, Vec::<String>::new(), rtl)
                    }
                }
                // PDFs render as-is (np.p5.books.pdf) — poppler rasterises each
                // page at its native layout; never reflowed into book mode.
                "pdf" => {
                    let total = books::comic_page_count(&pb);
                    if total > 0 { ("pdf-doc".to_string(), total, Vec::new(), false) }
                    else { ("pdf-empty".to_string(), 0, Vec::new(), false) }
                }
                "epub" | "mobi" | "azw3" | "fb2" => {
                    let chapters = match fmt {
                        "epub" => books::epub_chapters_text(&pb),
                        "mobi" | "azw3" => books::mobi_chapters_text(&pb),
                        _ => books::fb2_chapters_text(&pb),
                    };
                    if chapters.is_empty() {
                        (format!("{fmt}-empty"), 0, Vec::new(), false)
                    } else {
                        (fmt.to_string(), chapters.len(), chapters, false)
                    }
                }
                other => (other.to_string(), 0, Vec::new(), false),
            }
        }).await.unwrap_or(("".into(), 0, Vec::new(), false));
        let (format, total, chapters, rtl) = prep;
        let fmt2 = format.clone();
        let path2 = path.clone();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let comic = fmt_is_comic(&format);
            let mut state = tulipix_books::reader::ReaderState::new(total.max(1), comic, rtl);
            let start = saved_page.min(total.saturating_sub(1));
            let raster_doc = format == "pdf-doc";
            let chapter = if comic || raster_doc { 0 } else { start };
            if comic || raster_doc { state.page = start; }
            let mut sess = ReaderSession {
                item_id, path: path.clone(), format: format.clone(),
                comic: state, chapters, chapter,
                typo: tulipix_books::typography::Typography::default(),
                pages: Vec::new(), chapter_starts: Vec::new(), page: 0,
                page_w: 700.0, page_h: 900.0, turn: 0,
                guided: false, panel: 0, panels: Vec::new(), panels_page: usize::MAX,
                last_query: String::new(),
                last_left: String::new(),
                last_right: String::new(),
                read_since: std::time::Instant::now(),
            };
            // Text formats: build the initial pagination (a real reflow follows
            // once the text stage reports its true size) and restore saved page.
            if fmt_is_text(&sess.format) {
                reader_repaginate(&mut sess, false);
                sess.page = saved_page.min(sess.pages.len().saturating_sub(1));
                sess.chapter = chapter_of_page(&sess, sess.page);
            }
            READER.with(|r| *r.borrow_mut() = Some(sess));
            w.set_book_reader_title(
                std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("").into());
            w.set_book_toc_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<TocEntryRow>::new())));
            w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(Vec::<BookmarkRow>::new())));
            w.set_book_comic_thumbs(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::Image>::new())));
            reader_render(&w);
            w.set_book_reader_open(true);
        });
        // Comic timeline thumbs (np.p4.books.reader.navigation) — decoded off
        // the UI thread (disk-cached per page), pushed as one model when done.
        if (fmt_is_comic(&fmt2) || fmt2 == "pdf-doc") && total > 0 {
            let wk = weak.clone();
            tokio::task::spawn_blocking(move || {
                let thumbs: Vec<PathBuf> =
                    (0..total).filter_map(|i| books::comic_page_thumb(&path2, i)).collect();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    let imgs: Vec<slint::Image> = thumbs.iter()
                        .filter_map(|p| slint::Image::load_from_path(p).ok()).collect();
                    w.set_book_comic_thumbs(slint::ModelRc::new(slint::VecModel::from(imgs)));
                });
            });
        }
        // Load TOC and bookmarks async after opening
        if item_id >= 0 {
            let wk = weak.clone();
            let handle2 = tokio::runtime::Handle::current();
            handle2.spawn(async move {
                let Ok(pool) = pool_for("books").await else { return; };
                let toc_data = tulipix_books::navigation::toc(&pool, item_id).await.unwrap_or_default();
                let bm_data = tulipix_books::progress::bookmarks(&pool, item_id).await.unwrap_or_default();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    let toc_rows: Vec<TocEntryRow> = toc_data.into_iter().map(|e| TocEntryRow {
                        idx: e.idx as i32,
                        title: e.title.into(),
                        page: e.page.unwrap_or(0) as i32,
                    }).collect();
                    let bm_rows: Vec<BookmarkRow> = bm_data.into_iter().map(|(id, page, note, color)| BookmarkRow {
                        id: id as i32,
                        page: page as i32,
                        note: note.unwrap_or_default().into(),
                        color: color.unwrap_or_default().into(),
                    }).collect();
                    w.set_book_toc_entries(slint::ModelRc::new(slint::VecModel::from(toc_rows)));
                    w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(bm_rows)));
                });
            });
        }
    });
}
/// Guided view (np.p5.books.comic-guided): make sure the panel rects for the
/// current page are detected before rendering. Mutates the session, so it runs
/// as a separate borrow ahead of `reader_render`'s shared borrow.
fn reader_ensure_panels() {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if !(fmt_is_comic(&s.format) && s.guided) { return; }
        if s.panels_page == s.comic.page && !s.panels.is_empty() { return; }
        let rtl = s.comic.rtl;
        s.panels = books::comic_page_image(&s.path, s.comic.page, false)
            .map(|p| books::detect_panels(&p, rtl))
            .unwrap_or_default();
        s.panels_page = s.comic.page;
        s.panel = s.panel.min(s.panels.len().saturating_sub(1));
    });
}
fn reader_render(w: &MainWindow) {
    reader_ensure_panels();
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        match s.format.as_str() {
            f if fmt_is_comic(f) => {
                w.set_book_reader_kind("comic".into());
                let inv = s.comic.invert_images;
                if s.guided && !s.panels.is_empty() {
                    // One panel fills the stage; spread is ignored while guided.
                    let p = s.panel.min(s.panels.len() - 1);
                    let img = books::comic_panel_image(&s.path, s.comic.page, p, s.panels[p], inv);
                    w.set_book_page_left(img.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_page_right(slint::Image::default());
                    w.set_book_has_right(false);
                    w.set_book_page_label(format!(
                        "{} / {} · panel {} / {}",
                        s.comic.page + 1, s.comic.total, p + 1, s.panels.len()).into());
                } else {
                    let vis = s.comic.visible_pages();
                    let left = vis.first().and_then(|&i| books::comic_page_image(&s.path, i, inv));
                    let right = if matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double) {
                        vis.get(1).and_then(|&i| books::comic_page_image(&s.path, i, inv))
                    } else { None };
                    w.set_book_page_left(left.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_page_right(right.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_has_right(matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double) && vis.len() > 1);
                    w.set_book_page_label(format!("{} / {}", s.comic.page + 1, s.comic.total).into());
                }
                w.set_book_spread_double(matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double));
                w.set_book_rtl(s.comic.rtl);
                w.set_book_invert(s.comic.invert_images);
                w.set_book_guided(s.guided);
                w.set_book_progress(s.comic.fraction() as f32);
                w.set_book_page_index(s.comic.page as i32); // timeline highlight
                w.set_book_page_total(s.comic.total.max(1) as i32);
            }
            // PDF — continuous raster pages (np.p5.books.pdf). Pages lazy-load
            // via book-pdf-page-visible; here we set totals + progress and seed
            // an empty image model sized to the page count.
            "pdf-doc" => {
                w.set_book_reader_kind("pdf".into());
                let total = s.comic.total.max(1);
                let page = s.comic.page.min(total - 1);
                w.set_book_page_total(total as i32);
                w.set_book_page_index(page as i32);
                w.set_book_page_label(format!("{} / {}", page + 1, total).into());
                w.set_book_progress(s.comic.fraction() as f32);
                if w.get_book_pdf_pages().row_count() != total {
                    let empties: Vec<slint::Image> = (0..total).map(|_| slint::Image::default()).collect();
                    w.set_book_pdf_pages(slint::ModelRc::new(slint::VecModel::from(empties)));
                }
            }
            "epub" | "mobi" | "azw3" | "fb2" | "pdf" => {
                w.set_book_reader_kind("text".into());
                let total = s.pages.len().max(1);
                let page = s.page.min(total - 1);
                if w.get_book_read_mode() == "scroll" {
                    // Continuous mode (np.p4.books.continuous): whole chapter as
                    // one column; the nav bar steps/scrubs chapters.
                    let ctotal = s.chapters.len().max(1);
                    w.set_book_text(s.chapters.get(s.chapter).cloned().unwrap_or_default().into());
                    w.set_book_page_label(format!("Chapter {} / {}", s.chapter + 1, ctotal).into());
                    w.set_book_progress(s.chapter as f32 / (ctotal.saturating_sub(1).max(1) as f32));
                } else {
                    // Two-page spread: left/right faces; outgoing faces feed
                    // the flip overlay (np.p4.books.reader — real-book turn).
                    let two_up = w.get_book_two_up_active();
                    let left = s.pages.get(page).cloned().unwrap_or_default();
                    let right = if two_up {
                        s.pages.get(page + 1).cloned().unwrap_or_default()
                    } else { String::new() };
                    w.set_book_text_prev(std::mem::take(&mut s.last_left).into());
                    w.set_book_text_prev_right(std::mem::take(&mut s.last_right).into());
                    s.last_left = left.clone();
                    s.last_right = right.clone();
                    w.set_book_text(left.into());
                    w.set_book_text_right(right.into());
                    let label = if two_up && page + 1 < total {
                        format!("Ch {} · pages {}–{} / {}", s.chapter + 1, page + 1, page + 2, total)
                    } else {
                        format!("Ch {} · page {} / {}", s.chapter + 1, page + 1, total)
                    };
                    w.set_book_page_label(label.into());
                    w.set_book_progress(page as f32 / (total.saturating_sub(1).max(1) as f32));
                }
                w.set_book_page_index(page as i32);
                w.set_book_page_total(total as i32);
                w.set_book_font_px(s.typo.font_px as f32);
                w.set_book_line_height(s.typo.line_height as f32);
                w.set_book_line_height_val(s.typo.line_height as f32);
                w.set_book_margin_pct(s.typo.margin_pct as f32);
                w.set_book_font_family(family_to_int(s.typo.family));
                w.set_book_turn(s.turn);
            }
            _ => {
                w.set_book_reader_kind("unsupported".into());
                let label = match s.format.as_str() {
                    "cbr-empty" => "CBR needs unrar, bsdtar or 7z installed".to_string(),
                    "pdf-empty" => "Scanned PDF — install poppler-utils (pdftoppm) to view its pages".to_string(),
                    "djvu-empty" => "DjVu needs djvulibre (ddjvu) installed".to_string(),
                    f if f.ends_with("-empty") =>
                        format!("No readable text in this {} (DRM-protected?)",
                            f.trim_end_matches("-empty").to_uppercase()),
                    f => format!("{} files need a dedicated engine", f.to_uppercase()),
                };
                w.set_book_page_label(label.into());
            }
        }
    });
}
/// Persist the current reading position (page for comics / chapter for EPUB).
fn save_reader_progress() {
    let snap = READER.with(|r| {
        let mut g = r.borrow_mut();
        g.as_mut().map(|s| {
            let (page, total) = if s.comic.total > 0 && fmt_is_comic(&s.format) {
                (s.comic.page as i64, Some(s.comic.total as i64))
            } else {
                (s.page as i64, Some(s.pages.len().max(1) as i64))
            };
            // Flush the reading-time slice accumulated since the last save
            // (np.p5.books.stats) and restart the clock.
            let secs = s.read_since.elapsed().as_secs() as i64;
            s.read_since = std::time::Instant::now();
            (s.item_id, page, total, secs)
        })
    });
    let Some((item_id, page, total, secs)) = snap else { return; };
    if item_id < 0 { return; }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = tulipix_books::progress::save(&pool, item_id, &page.to_string(), page, total).await;
            // Cap a slice at 30 min so an overnight idle reader doesn't count.
            let _ = tulipix_books::progress::add_reading_time(&pool, item_id, secs.min(1800)).await;
        }
    });
}
fn reader_step(w: &MainWindow, dir: i32) {
    let scroll = w.get_book_read_mode() == "scroll";
    // Two-page spread turns two pages at once, like a real book.
    let step = if w.get_book_two_up_active() { 2usize } else { 1 };
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if fmt_is_comic(&s.format) && s.guided {
            // Guided view: panel → panel, overflowing to the next/prev page
            // (panels for the new page are detected by reader_ensure_panels;
            // usize::MAX clamps to the LAST panel when stepping backwards).
            if dir > 0 {
                if s.panel + 1 < s.panels.len() { s.panel += 1; }
                else if s.comic.page + 1 < s.comic.total {
                    s.comic.page += 1; s.panel = 0; s.panels_page = usize::MAX;
                }
            } else if s.panel > 0 { s.panel -= 1; }
            else if s.comic.page > 0 {
                s.comic.page -= 1; s.panel = usize::MAX; s.panels_page = usize::MAX;
            }
        } else if fmt_is_comic(&s.format) {
            if dir > 0 { s.comic.next(); } else { s.comic.prev(); }
        } else if fmt_is_text(&s.format) && scroll {
            // Continuous mode steps whole chapters.
            let c = if dir > 0 { (s.chapter + 1).min(s.chapters.len().saturating_sub(1)) }
                    else { s.chapter.saturating_sub(1) };
            s.chapter = c;
            s.page = s.chapter_starts.get(c).copied().unwrap_or(0);
        } else if fmt_is_text(&s.format) {
            let total = s.pages.len();
            if dir > 0 {
                if s.page + step < total { s.page += step; s.turn += 1; }
                else if s.page + 1 < total { s.page = total - 1; s.turn += 1; }
            } else if s.page > 0 {
                s.page = s.page.saturating_sub(step);
                s.turn += 1;
            }
            s.chapter = chapter_of_page(s, s.page);
        }
    });
    w.set_book_turn_dir(if dir > 0 { 1 } else { -1 });
    reader_render(w);
    save_reader_progress();
}
fn reader_scrub(w: &MainWindow, frac: f32) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if fmt_is_comic(&s.format) {
            s.comic.page = tulipix_books::navigation::scrub_to_page(frac as f64, s.comic.total);
            s.panel = 0;
        } else if fmt_is_text(&s.format) {
            let total = s.pages.len();
            s.page = tulipix_books::navigation::scrub_to_page(frac as f64, total);
            s.chapter = chapter_of_page(s, s.page);
        }
    });
    reader_render(w);
    save_reader_progress();
}
fn reader_toggle_spread(w: &MainWindow) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        if let Some(s) = g.as_mut() {
            use tulipix_books::reader::SpreadMode::*;
            s.comic.spread = match s.comic.spread { Single => Double, Double => Single };
        }
    });
    reader_render(w);
}
fn reader_toggle_rtl(w: &MainWindow) {
    READER.with(|r| { if let Some(s) = r.borrow_mut().as_mut() { s.comic.rtl = !s.comic.rtl; } });
    reader_render(w);
}
fn reader_toggle_invert(w: &MainWindow) {
    READER.with(|r| { if let Some(s) = r.borrow_mut().as_mut() { s.comic.toggle_invert(); } });
    reader_render(w);
}
fn reader_set_typo(w: &MainWindow, font: Option<f32>, line: Option<f32>, family: Option<i32>, margin: Option<f32>) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        if let Some(s) = g.as_mut() {
            if let Some(f) = font { s.typo.font_px = f as f64; }
            if let Some(l) = line { s.typo.line_height = l as f64; }
            if let Some(fam) = family { s.typo.family = int_to_family(fam); }
            if let Some(m) = margin { s.typo.margin_pct = m as f64; }
            s.typo = s.typo.clamped();
            // Font/line/family changes alter how much text fits a page; reflow,
            // keeping the reader near the same spot. (Margin reflows via the
            // text-stage resize callback, but repaginate here too for keyboard.)
            if fmt_is_text(&s.format) { reader_repaginate(s, true); }
        }
    });
    reader_render(w);
}
/// The text stage reported a new size — re-paginate EPUB to fill it exactly.
fn reader_reflow(w: &MainWindow, width: f32, height: f32) {
    let changed = READER.with(|r| {
        let mut g = r.borrow_mut();
        match g.as_mut() {
            Some(s) if fmt_is_text(&s.format) && width > 8.0 && height > 8.0
                && ((s.page_w - width).abs() > 1.0 || (s.page_h - height).abs() > 1.0) => {
                s.page_w = width;
                s.page_h = height;
                reader_repaginate(s, true);
                true
            }
            _ => false,
        }
    });
    if changed { reader_render(w); }
}
