//! The Studio section's store, and the parts of making a movie or a book that
//! need no ffmpeg: which photos to use, how long each stays on screen, and the
//! ffmpeg command lines themselves. `api::studio` runs them.
//!
//! Not `tulipix_photos::slideshow`: that plan loops each photo into zoompan
//! (which multiplies frames), stretches every photo to the frame, gives xfade
//! mismatched frame rates and runs the system ffmpeg. Nothing called it.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id        INTEGER PRIMARY KEY,
    title     TEXT    NOT NULL,
    kind      TEXT    NOT NULL DEFAULT 'movie',  -- movie | book
    source    TEXT    NOT NULL DEFAULT '',       -- trip:3 | album:7 | person:2 | range
    start     INTEGER NOT NULL DEFAULT 0,
    end       INTEGER NOT NULL DEFAULT 0,
    length    TEXT    NOT NULL DEFAULT 'medium', -- short | medium | long | song
    shape     TEXT    NOT NULL DEFAULT 'wide',   -- wide | tall | square
    music_id  INTEGER NOT NULL DEFAULT 0,        -- a Music item; 0 is none
    beat      INTEGER NOT NULL DEFAULT 1,
    picks     TEXT    NOT NULL DEFAULT '',       -- Photos item ids, in order
    out_path  TEXT    NOT NULL DEFAULT '',
    state     TEXT    NOT NULL DEFAULT 'draft',  -- draft | queued | rendering | done | failed
    progress  REAL    NOT NULL DEFAULT 0,
    error     TEXT    NOT NULL DEFAULT '',
    created   INTEGER NOT NULL,
    rendered  INTEGER NOT NULL DEFAULT 0
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── picking ─────────────────────────────────────────────────────────────────

/// A photo that could go in.
#[derive(Debug, Clone, PartialEq)]
pub struct Cand {
    pub id: i64,
    pub ts: i64,
    /// A local day number, for spreading picks across the days.
    pub day: i64,
    pub starred: bool,
    pub phash: Option<u64>,
}

/// Photos that are not the same moment twice: a burst, or the same view taken
/// again, keeps its first (or its starred one).
pub fn distinct(c: &[Cand]) -> Vec<Cand> {
    let mut out: Vec<Cand> = Vec::new();
    for x in c {
        if let Some(last) = out.last_mut() {
            let same_moment = x.ts - last.ts < 30;
            let same_view = matches!((x.phash, last.phash), (Some(a), Some(b)) if (a ^ b).count_ones() <= 8);
            if same_moment || (same_view && x.ts - last.ts < 600) {
                if x.starred && !last.starred {
                    *last = x.clone();
                }
                continue;
            }
        }
        out.push(x.clone());
    }
    out
}

/// Up to `n` photos spread over the days they cover: each day gets a share
/// by how many it has (at least one while there is room), starred first, the
/// rest evenly through the day. The result is in time order.
pub fn pick(c: &[Cand], n: usize) -> Vec<i64> {
    let c = distinct(c);
    if c.len() <= n {
        return c.iter().map(|x| x.id).collect();
    }
    let mut days: BTreeMap<i64, Vec<&Cand>> = BTreeMap::new();
    for x in &c {
        days.entry(x.day).or_default().push(x);
    }
    // Shares by size, largest remainder; days beyond `n` get none.
    let total = c.len() as f64;
    let mut share: Vec<(i64, usize, f64)> = days
        .iter()
        .map(|(d, v)| {
            let exact = v.len() as f64 / total * n as f64;
            (*d, exact.floor() as usize, exact - exact.floor())
        })
        .collect();
    // Every day at least one, while there are slots.
    let mut used: usize = share.iter().map(|s| s.1).sum();
    for s in share.iter_mut() {
        if s.1 == 0 && used < n {
            s.1 = 1;
            used += 1;
        }
    }
    let mut order: Vec<usize> = (0..share.len()).collect();
    order.sort_by(|&a, &b| share[b].2.total_cmp(&share[a].2));
    for i in order {
        if used >= n {
            break;
        }
        if share[i].1 < days[&share[i].0].len() {
            share[i].1 += 1;
            used += 1;
        }
    }
    let mut out: Vec<&Cand> = Vec::new();
    for (d, k, _) in share {
        let v = &days[&d];
        let k = k.min(v.len());
        if k == 0 {
            continue;
        }
        let mut chosen: Vec<&Cand> = v.iter().filter(|x| x.starred).take(k).copied().collect();
        let rest: Vec<&Cand> = v.iter().filter(|x| !x.starred).copied().collect();
        let need = k - chosen.len();
        if need > 0 && !rest.is_empty() {
            // Evenly through the day.
            for j in 0..need {
                let at = (j * rest.len()) / need + rest.len() / (2 * need);
                chosen.push(rest[at.min(rest.len() - 1)]);
            }
        }
        out.extend(chosen);
    }
    out.sort_by_key(|x| x.ts);
    out.dedup_by_key(|x| x.id);
    out.into_iter().take(n).map(|x| x.id).collect()
}

// ── timing ──────────────────────────────────────────────────────────────────

/// Seconds a length asks for; a song's own when it is "song".
pub fn target_secs(length: &str, song_secs: f64) -> f64 {
    match length {
        "short" => 30.0,
        "long" => 180.0,
        "song" if song_secs > 20.0 => song_secs.min(420.0),
        _ => 60.0,
    }
}

/// How long each photo stays: a whole number of beats near 3.5 s when the
/// song's tempo is known and the cut should fall on it, else 3.5 s.
pub fn slide_secs(bpm: Option<f64>, on_beat: bool) -> f64 {
    match bpm.filter(|b| on_beat && (40.0..=240.0).contains(b)) {
        Some(b) => {
            let beat = 60.0 / b;
            let beats = (3.5 / beat).round().max(2.0);
            // Bars of four read better than odd counts.
            let beats = if beats >= 4.0 { (beats / 4.0).round() * 4.0 } else { beats };
            beat * beats
        }
        None => 3.5,
    }
}

pub const XFADE: f64 = 0.6;

/// How many photos fill `target` seconds at `slide` each, with crossfades.
pub fn photos_for(target: f64, slide: f64) -> usize {
    (((target - XFADE) / (slide - XFADE)).floor() as usize).max(2)
}

// ── ffmpeg ──────────────────────────────────────────────────────────────────

/// Output size for a shape.
pub fn frame(shape: &str) -> (u32, u32) {
    match shape {
        "tall" => (1080, 1920),
        "square" => (1080, 1080),
        _ => (1920, 1080),
    }
}

/// EXIF orientation as filters; ffmpeg is told not to rotate on its own, so
/// this is the only turn a photo gets.
fn turn(orientation: i64) -> &'static str {
    match orientation {
        3 => "hflip,vflip,",
        6 => "transpose=1,",
        8 => "transpose=2,",
        _ => "",
    }
}

pub struct Slide {
    pub path: String,
    pub orientation: i64,
}

/// The ffmpeg arguments for a movie: every photo filling the frame with a slow
/// push in, crossfades between, the song trimmed and faded to the length.
/// `-progress pipe:1` reports how far it is.
pub fn movie_args(slides: &[Slide], music: Option<&str>, shape: &str, slide: f64, out: &str) -> Result<Vec<String>> {
    if slides.len() < 2 {
        bail!("a movie needs at least two photos");
    }
    let (w, h) = frame(shape);
    let (bw, bh) = ((w as f64 * 1.2) as u32 / 2 * 2, (h as f64 * 1.2) as u32 / 2 * 2);
    const FPS: u32 = 30;
    let n = slides.len();
    let total = n as f64 * slide - (n - 1) as f64 * XFADE;
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"].map(String::from).to_vec();
    for s in slides {
        a.extend(["-noautorotate", "-loop", "1", "-framerate"].map(String::from));
        a.push(FPS.to_string());
        a.push("-t".into());
        a.push(format!("{slide:.3}"));
        a.push("-i".into());
        a.push(s.path.clone());
    }
    if let Some(m) = music {
        a.push("-i".into());
        a.push(m.to_string());
    }
    let mut g = String::new();
    for (i, s) in slides.iter().enumerate() {
        // Alternate a push in from the middle with one drifting to a corner.
        let (x, y) = if i % 2 == 0 {
            ("iw/2-(iw/zoom/2)", "ih/2-(ih/zoom/2)")
        } else {
            ("(iw-iw/zoom)*on/(on+60)", "(ih-ih/zoom)*on/(on+90)")
        };
        g.push_str(&format!(
            "[{i}:v]{}scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},\
             zoompan=z='min(zoom+0.0008,1.12)':x='{x}':y='{y}':d=1:s={w}x{h}:fps={FPS},setsar=1,format=yuv420p[v{i}];",
            turn(s.orientation)
        ));
    }
    let mut prev = "v0".to_string();
    for i in 1..n {
        let next = if i == n - 1 { "vout".to_string() } else { format!("x{i}") };
        let offset = i as f64 * (slide - XFADE);
        g.push_str(&format!("[{prev}][v{i}]xfade=transition=fade:duration={XFADE}:offset={offset:.3}[{next}];"));
        prev = next;
    }
    if music.is_some() {
        let fade = (total - 2.5).max(0.0);
        g.push_str(&format!("[{n}:a]atrim=0:{total:.3},asetpts=PTS-STARTPTS,afade=t=in:d=0.4,afade=t=out:st={fade:.3}:d=2.5[aout];"));
    }
    a.push("-filter_complex".into());
    a.push(g.trim_end_matches(';').to_string());
    a.extend(["-map", "[vout]"].map(String::from));
    if music.is_some() {
        a.extend(["-map", "[aout]", "-c:a", "aac", "-b:a", "192k"].map(String::from));
    }
    a.extend(
        ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]
            .map(String::from),
    );
    a.extend(["-t".to_string(), format!("{total:.3}"), "-progress".into(), "pipe:1".into(), "-nostats".into()]);
    a.push(out.to_string());
    Ok(a)
}

/// Seconds a movie of `n` photos runs.
pub fn movie_secs(n: usize, slide: f64) -> f64 {
    n as f64 * slide - n.saturating_sub(1) as f64 * XFADE
}

/// A photo book's pages: one photo, or two of the same shape side by side (tall
/// ones) or stacked (wide ones). `wide[i]` says whether photo `i` is wider than
/// it is tall.
pub fn pages(wide: &[bool]) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < wide.len() {
        // Every fourth page a photo alone, so the book breathes.
        let alone = out.len() % 4 == 0;
        if !alone && i + 1 < wide.len() && wide[i] == wide[i + 1] {
            out.push(vec![i, i + 1]);
            i += 2;
        } else {
            out.push(vec![i]);
            i += 1;
        }
    }
    out
}

/// Book pages are 21 cm square at 300 dpi.
pub const PAGE: u32 = 2480;
const MARGIN: u32 = 150;
const GUTTER: u32 = 70;

/// The ffmpeg arguments for one page: the photos laid on white, each fitted
/// into its cell, written as a JPEG.
pub fn page_args(slides: &[Slide], wide: bool, out: &str) -> Vec<String> {
    let inner = PAGE - 2 * MARGIN;
    let half = (inner - GUTTER) / 2;
    let cells: Vec<(u32, u32, u32, u32)> = match slides.len() {
        1 => vec![(MARGIN, MARGIN, inner, inner)],
        _ if wide => vec![(MARGIN, MARGIN, inner, half), (MARGIN, MARGIN + half + GUTTER, inner, half)],
        _ => vec![(MARGIN, MARGIN, half, inner), (MARGIN + half + GUTTER, MARGIN, half, inner)],
    };
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"].map(String::from).to_vec();
    for s in slides {
        a.extend(["-noautorotate", "-i"].map(String::from));
        a.push(s.path.clone());
    }
    let mut g = format!("color=c=white:s={PAGE}x{PAGE}:d=1[p0];");
    for (i, (s, (x, y, cw, ch))) in slides.iter().zip(&cells).enumerate() {
        g.push_str(&format!(
            "[{i}:v]{}scale={cw}:{ch}:force_original_aspect_ratio=decrease[i{i}];\
             [p{i}][i{i}]overlay=x={x}+({cw}-overlay_w)/2:y={y}+({ch}-overlay_h)/2[p{}];",
            turn(s.orientation),
            i + 1
        ));
    }
    a.push("-filter_complex".into());
    a.push(g.trim_end_matches(';').to_string());
    a.extend(["-map".to_string(), format!("[p{}]", slides.len()), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into()]);
    a.push(out.to_string());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: i64, ts: i64, day: i64, starred: bool) -> Cand {
        Cand { id, ts, day, starred, phash: None }
    }

    #[test]
    fn bursts_collapse_and_days_share_the_picks() {
        // A burst of three keeps its starred one.
        let burst = vec![c(1, 0, 0, false), c(2, 5, 0, true), c(3, 9, 0, false)];
        assert_eq!(distinct(&burst).iter().map(|x| x.id).collect::<Vec<_>>(), vec![2]);
        // Day 0 has 30 photos, day 1 has 10: eight picks go 6 and 2.
        let mut all: Vec<Cand> = (0..30).map(|i| c(i, i * 100, 0, false)).collect();
        all.extend((0..10).map(|i| c(100 + i, 100_000 + i * 100, 1, i == 3)));
        let p = pick(&all, 8);
        assert_eq!(p.len(), 8);
        assert_eq!(p.iter().filter(|&&id| id >= 100).count(), 2);
        assert!(p.contains(&103), "the starred photo goes in");
        let mut sorted = p.clone();
        sorted.sort();
        assert_eq!(p, sorted, "picks stay in time order");
        // Fewer photos than asked for: all of them.
        assert_eq!(pick(&all[..3], 8).len(), 3);
    }

    #[test]
    fn slides_fall_on_the_beat() {
        // 120 bpm is half a second a beat: 3.5 s is 7 beats, a bar of 8 is 4 s.
        assert!((slide_secs(Some(120.0), true) - 4.0).abs() < 1e-9);
        assert!((slide_secs(Some(120.0), false) - 3.5).abs() < 1e-9);
        assert!((slide_secs(None, true) - 3.5).abs() < 1e-9);
        assert_eq!(photos_for(60.0, 3.5), 20);
        assert!((movie_secs(20, 3.5) - 58.6).abs() < 1e-9);
    }

    #[test]
    fn a_movie_command_line() {
        let s = |p: &str, o| Slide { path: p.into(), orientation: o };
        let a = movie_args(&[s("/a.jpg", 1), s("/b.jpg", 6), s("/c.jpg", 1)], Some("/m.mp3"), "tall", 4.0, "/o.mp4").unwrap();
        let g = &a[a.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert_eq!(g.matches("xfade").count(), 2);
        assert!(g.contains("[1:v]transpose=1,scale=1296:2304"));
        assert!(g.contains("s=1080x1920"));
        assert!(g.contains("[3:a]atrim=0:10.800"));
        assert!(a.windows(2).any(|w| w[0] == "-map" && w[1] == "[aout]"));
        assert!(movie_args(&[s("/a.jpg", 1)], None, "wide", 4.0, "/o.mp4").is_err());
    }

    #[test]
    fn book_pages_pair_like_with_like() {
        let p = pages(&[true, true, true, false, false, true]);
        assert_eq!(p, vec![vec![0], vec![1, 2], vec![3, 4], vec![5]]);
        let a = page_args(&[Slide { path: "/a.jpg".into(), orientation: 1 }, Slide { path: "/b.jpg".into(), orientation: 1 }], true, "/p.jpg");
        let g = &a[a.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(g.contains("scale=2180:1055"));
        assert!(a.windows(2).any(|w| w[0] == "-map" && w[1] == "[p2]"));
    }
}
