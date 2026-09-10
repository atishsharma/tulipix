//! Download pipeline — port of mdl `sync.ts` + `youtube.ts` + `metadata.ts`.
//!
//! Per track: yt-dlp searches YouTube and extracts the audio, then lofty writes
//! the provider metadata over yt-dlp's guess — title, artist, album, album
//! artist, track number and cover art.
//!
//! The search is weighed rather than taken: five hits come back with their
//! lengths and the one closest to the provider's duration wins, because
//! `ytsearch1` is how a cover, a live take or a ten-hour loop ends up in the
//! library wearing the real track's name and artwork.

use crate::manifest::{self, Manifest, ManifestTrack};
use crate::types::{DownloadOptions, NameMethod, Playlist, Progress, Stage, Summary, Track};
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use tulipix_core::proc::NoWindow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Filesystem-safe file stem for `track` at 1-based `index`, per `method`.
pub fn track_file_stem(index: usize, track: &Track, method: NameMethod) -> String {
    sanitize_filename::sanitize(method.stem(index, track))
}

fn search_query(track: &Track) -> String {
    format!("{} - {}", track.artists.join(", "), track.title)
}

fn ytdlp_bin() -> PathBuf {
    tulipix_core::ytdlp::bin()
}

/// How many times to (re)try a single track before giving up. YouTube throws
/// transient 403/network hiccups; a couple of retries clears most of them.
const MAX_TRIES: usize = 3;

/// How many search hits to weigh before picking one.
const CANDIDATES: usize = 5;

/// How far a hit may be from the length the provider gave: twenty seconds, or
/// 8% of the track, whichever is more forgiving. Wide enough for a fade-out
/// trimmed differently, a radio edit's count-in, or a live album's applause;
/// far too tight for a ten-hour loop, a full-album upload, a thirty-second
/// preview or an extended mix.
fn tolerance_s(target_s: f64) -> f64 {
    20.0_f64.max(target_s * 0.08)
}

/// Choose the search hit whose length is closest to what the provider said the
/// track is, out of `candidates` of `(video_id, duration_s)`.
///
/// `ytsearch1` — taking whatever YouTube ranks first — is how a cover, a live
/// take, a reaction video or a ten-hour loop ends up in the library tagged as
/// the real thing. The provider already told us how long the track is; this is
/// the check that was never made.
///
/// `target_s <= 0` means the provider did not say, and then the first hit is
/// still the best guess available. Otherwise a hit outside [`tolerance_s`] is
/// not returned at all: failing the track is recoverable, and silently
/// downloading the wrong audio under the right name is not.
fn pick_by_duration(candidates: &[(String, f64)], target_s: f64) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    if target_s <= 0.0 {
        return Some(candidates[0].0.clone());
    }
    let tol = tolerance_s(target_s);
    candidates
        .iter()
        .filter(|(_, d)| *d > 0.0 && (*d - target_s).abs() <= tol)
        .min_by(|a, b| {
            (a.1 - target_s)
                .abs()
                .total_cmp(&(b.1 - target_s).abs())
        })
        .map(|(id, _)| id.clone())
}

/// Parse the `id|duration` lines `--print` emits. yt-dlp writes `NA` for a
/// duration it does not know, which parses to 0 and is then only usable as a
/// fallback when the provider gave no length either.
fn parse_candidates(stdout: &str) -> Vec<(String, f64)> {
    stdout
        .lines()
        .filter_map(|l| {
            let (id, dur) = l.trim().split_once('|')?;
            (!id.is_empty()).then(|| (id.to_string(), dur.trim().parse().unwrap_or(0.0)))
        })
        .collect()
}

/// Ask YouTube for `CANDIDATES` hits and their lengths, without downloading
/// anything. `--flat-playlist` keeps this to the one search request.
async fn search_candidates(query: &str) -> Vec<(String, f64)> {
    let mut cmd = tokio::process::Command::new(ytdlp_bin());
    cmd.arg(format!("ytsearch{CANDIDATES}:{query}"))
        .args(["--flat-playlist", "--no-warnings", "--print", "%(id)s|%(duration)s"]);
    for a in tulipix_core::ytdlp::common_args() {
        cmd.arg(a);
    }
    cmd.no_window();
    match cmd.output().await {
        Ok(out) if out.status.success() => {
            parse_candidates(&String::from_utf8_lossy(&out.stdout))
        }
        Ok(out) => {
            tracing::debug!(stderr = %String::from_utf8_lossy(&out.stderr), "mdl: candidate search failed");
            Vec::new()
        }
        Err(e) => {
            tracing::debug!(error = %e, "mdl: candidate search could not run");
            Vec::new()
        }
    }
}

/// Download one track's audio into `dest`. Returns the produced path.
///
/// Searches YouTube for "artist - title" (per track, never the source
/// playlist), picks the hit whose length matches what the provider said, and
/// extracts audio via ffmpeg.
#[allow(clippy::too_many_arguments)]
async fn download_audio(
    track: &Track,
    index: usize,
    dest: &Path,
    format: &str,
    bitrate: u32,
    method: NameMethod,
    threads: usize,
) -> Result<PathBuf> {
    let stem = track_file_stem(index, track, method);
    let out_tmpl = dest.join(format!("{stem}.%(ext)s"));
    let ffmpeg = tulipix_core::thumbs::tool_bin("ffmpeg");
    let path = dest.join(format!("{stem}.{format}"));

    // Weigh a handful of hits against the length the provider gave, then
    // download the one that matches, rather than whatever YouTube ranked
    // first. The search costs one flat request and is done once — the retry
    // loop below re-downloads the chosen video instead of re-searching, so a
    // transient 403 cannot quietly land on a different result second time.
    let search = search_query(track);
    let target_s = track.duration_ms.map(|ms| ms as f64 / 1000.0).unwrap_or(0.0);
    let candidates = search_candidates(&search).await;
    let query = match pick_by_duration(&candidates, target_s) {
        Some(id) => format!("https://www.youtube.com/watch?v={id}"),
        // Nothing was close enough. Only a real mismatch reaches here: an
        // empty candidate list falls back to the old one-shot search, because
        // no answer at all is a network problem, not a wrong answer.
        None if !candidates.is_empty() && target_s > 0.0 => {
            bail!(
                "no YouTube result within {:.0}s of the expected {:.0}s — \
                 refusing to guess",
                tolerance_s(target_s),
                target_s
            );
        }
        None => format!("ytsearch1:{search}"),
    };

    let mut last_err = anyhow!("download failed");
    for attempt in 1..=MAX_TRIES {
        let mut cmd = tokio::process::Command::new(ytdlp_bin());
        cmd.arg(&query)
            .args(["-f", "bestaudio/best", "-x", "--audio-format", format])
            .args(["--no-playlist", "--no-warnings"]);
        // Target bitrate for lossy codecs — passed to the ffmpeg re-encode.
        // Lossless (flac/wav) ignores it, so only set for opus/m4a/mp3.
        if bitrate > 0 && matches!(format, "opus" | "m4a" | "mp3") {
            cmd.args(["--audio-quality", &format!("{bitrate}K")]);
        }
        // Threads per download — parallel fragment downloads for this track.
        cmd.args(["--concurrent-fragments", &threads.max(1).to_string()]);
        // NOTE: cover art is embedded by write_tags (below) from each provider's
        // high-res artwork_url — Spotify/Apple/YT-Music square art, or a YouTube
        // video's highest-quality thumbnail. yt-dlp's own --embed-thumbnail is
        // deliberately NOT used: it embeds a SECOND picture into the opus, and the
        // double cover breaks ffmpeg's `-map 0:v?` art extraction (Spotify albums
        // downloaded with no art). The thumb renderer center-crops to square, which
        // is a no-op on already-square provider art and squares a 16:9 video thumb.
        // yt-dlp needs ffmpeg for the Opus extraction. Point it at the resolved
        // binary so it works even when ffmpeg isn't on the app process's PATH.
        if ffmpeg.exists() {
            cmd.arg("--ffmpeg-location").arg(&ffmpeg);
        }
        // Cookies (Settings → "yt-dlp cookies") and any player-client override,
        // from the one place that knows about both. The client rotation this
        // used to hardcode is off by default now — see
        // `tulipix_core::ytdlp::player_client_args`.
        for a in tulipix_core::ytdlp::common_args() {
            cmd.arg(a);
        }
        cmd.arg("-o").arg(&out_tmpl);
        // Suppress the console window Windows would otherwise flash for every
        // yt-dlp/ffmpeg child of this GUI app. No-op off Windows.
        cmd.no_window();

        tracing::info!(query = %query, attempt, dest = %dest.display(), "mdl: yt-dlp search+download");
        match cmd.output().await {
            Err(e) => {
                tracing::warn!(error = %e, "mdl: yt-dlp spawn failed (is yt-dlp installed?)");
                return Err(anyhow!("yt-dlp could not be launched: {e}"));
            }
            Ok(out) if out.status.success() => {
                if path.exists() {
                    return Ok(path);
                }
                tracing::warn!(expected = %path.display(), stdout = %String::from_utf8_lossy(&out.stdout), "mdl: expected output not produced");
                last_err = anyhow!("no audio file produced (ffmpeg missing?)");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::warn!(%stderr, attempt, "mdl: yt-dlp exited non-zero");
                // A refusal never clears by retrying — fail fast, and say the
                // two things that actually fix it.
                if tulipix_core::ytdlp::is_access_error(&stderr) {
                    bail!("{}", tulipix_core::ytdlp::friendly_error(&stderr));
                }
                last_err = anyhow!("yt-dlp: {}", tulipix_core::ytdlp::friendly_error(&stderr));
            }
        }
        if attempt < MAX_TRIES {
            tokio::time::sleep(std::time::Duration::from_millis(800 * attempt as u64)).await;
        }
    }
    Err(last_err)
}

/// The album artist for a whole download: the one name every track shares, or
/// "Various Artists" when they do not.
///
/// The library groups albums by album artist. Without one, a compilation
/// arrives as a dozen one-track albums under a dozen artists, which is the
/// same album split rather than the album.
fn album_artist_of(playlist: &Playlist) -> String {
    let mut names = playlist.tracks.iter().filter_map(|t| t.artists.first());
    let Some(first) = names.next() else { return String::new() };
    if names.all(|a| a == first) {
        first.clone()
    } else {
        "Various Artists".to_string()
    }
}

/// Overwrite tags + cover art with the provider metadata via lofty.
///
/// `index` is the track's 1-based place in `total`, and `album_artist` is
/// [`album_artist_of`] for the whole run. Both matter after the download is
/// over: an album with no track numbers sorts alphabetically in the library
/// for ever, and one with no album artist does not stay one album.
async fn write_tags(
    path: &Path,
    track: &Track,
    index: usize,
    total: usize,
    album_artist: &str,
    client: &reqwest::Client,
) -> Result<()> {
    // Fetch cover art up front (async), hand the bytes to the blocking tagger.
    let cover: Option<Vec<u8>> = match &track.artwork_url {
        Some(u) => match client.get(u).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => resp.bytes().await.ok().map(|b| b.to_vec()),
                Err(_) => None,
            },
            Err(_) => None,
        },
        None => None,
    };

    let path = path.to_path_buf();
    let title = track.title.clone();
    let artist = track.artists.join(", ");
    let album = track.album.clone();
    let album_artist = album_artist.to_string();

    tokio::task::spawn_blocking(move || -> Result<()> {
        use lofty::config::WriteOptions;
        use lofty::file::TaggedFileExt;
        use lofty::picture::{MimeType, Picture, PictureType};
        use lofty::prelude::{Accessor, ItemKey, TagExt};
        use lofty::tag::Tag;

        let mut tagged = lofty::read_from_path(&path)?;
        if tagged.primary_tag().is_none() {
            let tt = tagged.primary_tag_type();
            tagged.insert_tag(Tag::new(tt));
        }
        let tag = tagged
            .primary_tag_mut()
            .ok_or_else(|| anyhow!("could not obtain a primary tag"))?;

        tag.set_title(title);
        tag.set_artist(artist);
        if let Some(a) = album {
            tag.set_album(a);
        }
        // The provider handed over an ordered list; this is where that order
        // stops being only a filename prefix.
        tag.set_track(index as u32);
        if total > 0 {
            tag.set_track_total(total as u32);
        }
        if !album_artist.is_empty() {
            tag.insert_text(ItemKey::AlbumArtist, album_artist);
        }
        if let Some(bytes) = cover {
            // Detect the real image type from the magic bytes — provider covers
            // are JPEG (Spotify/Apple) or PNG/WebP (YouTube). A mislabeled mime
            // makes some players drop the art, so never assume JPEG.
            let mime = match bytes.as_slice() {
                [0x89, b'P', b'N', b'G', ..] => MimeType::Png,
                [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => {
                    MimeType::Unknown("image/webp".into())
                }
                _ => MimeType::Jpeg,
            };
            // lofty 0.25 turned `new_unchecked` into a builder. Same four
            // fields, no description, and `unchecked` still means the mime we
            // sniffed above is taken at its word rather than re-derived.
            let pic = Picture::unchecked(bytes)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build();
            tag.push_picture(pic);
        }
        tag.save_to_path(&path, WriteOptions::default())?;
        Ok(())
    })
    .await??;
    Ok(())
}

/// Download a whole playlist. `on_progress` fires on every stage transition.
/// Set `cancel` to abort remaining/queued tracks.
pub async fn download_playlist<F>(
    client: &reqwest::Client,
    playlist: &Playlist,
    opts: &DownloadOptions,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Summary
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    let on_progress = Arc::new(on_progress);
    let _ = tokio::fs::create_dir_all(&opts.dest_dir).await;
    tracing::info!(tracks = playlist.tracks.len(), dest = %opts.dest_dir.display(), "mdl: starting playlist download");

    let manifest = Arc::new(Mutex::new(manifest::load(&opts.dest_dir).unwrap_or_else(
        || Manifest::new(playlist.provider, &playlist.id, &playlist.title, &playlist.source_url),
    )));
    let total = playlist.tracks.len();
    let sem = Arc::new(Semaphore::new(opts.parallelism.max(1)));
    // (downloaded, skipped, failed)
    let counters = Arc::new(Mutex::new((0usize, 0usize, Vec::<(Track, String)>::new())));
    let format = opts.format.clone();
    let bitrate = opts.bitrate;
    let method = opts.name_method;
    let threads = opts.threads_per_download.max(1);
    // Decided once for the whole run, not per track — that is what makes it an
    // album artist rather than an artist.
    let album_artist = album_artist_of(playlist);

    let mut handles = Vec::new();
    for (i, track) in playlist.tracks.iter().cloned().enumerate() {
        let sem = sem.clone();
        let client = client.clone();
        let dest = opts.dest_dir.clone();
        let cancel = cancel.clone();
        let on_progress = on_progress.clone();
        let counters = counters.clone();
        let manifest = manifest.clone();
        let format = format.clone();
        let album_artist = album_artist.clone();

        let already = manifest
            .lock()
            .unwrap()
            .contains(&track.id)
            .map(|m| dest.join(&m.relative_path))
            .filter(|p| p.exists())
            .is_some();

        handles.push(tokio::spawn(async move {
            let _permit = match sem.acquire().await {
                Ok(p) => p,
                Err(_) => return,
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let idx = i + 1;
            let emit = {
                let counters = counters.clone();
                let on_progress = on_progress.clone();
                let title = track.title.clone();
                move |stage: Stage, percent: f32, message: String, file: Option<String>| {
                    let (d, s, f) = {
                        let c = counters.lock().unwrap();
                        (c.0, c.1, c.2.len())
                    };
                    on_progress(Progress {
                        track_index: idx,
                        total,
                        downloaded: d,
                        skipped: s,
                        failed: f,
                        stage,
                        percent,
                        message,
                        title: title.clone(),
                        file_name: file,
                    });
                }
            };

            if already {
                counters.lock().unwrap().1 += 1;
                emit(Stage::Skipped, 100.0, "Already downloaded".into(), None);
                return;
            }

            emit(Stage::SearchingYoutube, 20.0, "Searching YouTube".into(), None);
            match download_audio(&track, idx, &dest, &format, bitrate, method, threads).await {
                Ok(path) => {
                    emit(Stage::WritingMetadata, 90.0, "Embedding metadata".into(), None);
                    if let Err(e) =
                        write_tags(&path, &track, idx, total, &album_artist, &client).await
                    {
                        tracing::warn!(error = %e, "mdl: tag write failed");
                    }
                    let file_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    counters.lock().unwrap().0 += 1;
                    // Persist to manifest immediately so a mid-run cancel still resyncs.
                    {
                        let mut m = manifest.lock().unwrap();
                        m.upsert(ManifestTrack::from_track(&track, &file_name));
                        let _ = manifest::save(&dest, &m);
                    }
                    emit(Stage::Completed, 100.0, file_name.clone(), Some(file_name));
                }
                Err(e) => {
                    counters.lock().unwrap().2.push((track.clone(), e.to_string()));
                    emit(Stage::Failed, 100.0, e.to_string(), None);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let (downloaded, skipped, failed) = {
        let c = counters.lock().unwrap();
        (c.0, c.1, c.2.clone())
    };
    Summary { downloaded, skipped, failed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Track;

    #[test]
    fn builds_sanitized_filename() {
        let t = Track {
            id: "1".into(),
            title: "Song/Name".into(),
            artists: vec!["A".into(), "B".into()],
            album: Some("Alb".into()),
            artwork_url: None,
            duration_ms: None,
            source_url: None,
        };
        assert_eq!(track_file_stem(3, &t, NameMethod::Numbered), "03 - A - SongName");
        assert_eq!(track_file_stem(3, &t, NameMethod::ArtistsSong), "A, B - SongName");
        assert_eq!(track_file_stem(3, &t, NameMethod::AlbumSong), "Alb - SongName");
        assert_eq!(track_file_stem(3, &t, NameMethod::ArtistSong), "A - SongName");
        assert_eq!(track_file_stem(3, &t, NameMethod::SongOnly), "SongName");
    }

    fn track(id: &str, artists: &[&str]) -> Track {
        Track {
            id: id.into(),
            title: "T".into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album: None,
            artwork_url: None,
            duration_ms: None,
            source_url: None,
        }
    }

    fn playlist(tracks: Vec<Track>) -> Playlist {
        Playlist {
            id: "p".into(),
            title: "P".into(),
            owner: None,
            artwork_url: None,
            provider: crate::types::ProviderId::Spotify,
            source_url: "https://example.invalid".into(),
            tracks,
        }
    }

    #[test]
    fn a_hit_of_the_wrong_length_is_not_the_track() {
        // 3:30. The loop and the preview are the two shapes that used to get
        // through, because YouTube ranks both above the real upload often
        // enough to matter.
        let hits = vec![
            ("loop".to_string(), 36_000.0),
            ("preview".to_string(), 30.0),
            ("real".to_string(), 212.0),
            ("extended".to_string(), 480.0),
        ];
        assert_eq!(pick_by_duration(&hits, 210.0).as_deref(), Some("real"));
    }

    #[test]
    fn nothing_close_enough_is_no_answer_at_all() {
        let hits = vec![("loop".to_string(), 36_000.0), ("preview".to_string(), 30.0)];
        assert_eq!(pick_by_duration(&hits, 210.0), None, "better to fail than to guess");
    }

    #[test]
    fn without_a_provider_duration_the_first_hit_still_wins() {
        let hits = vec![("first".to_string(), 0.0), ("second".to_string(), 210.0)];
        assert_eq!(pick_by_duration(&hits, 0.0).as_deref(), Some("first"));
        assert_eq!(pick_by_duration(&[], 210.0), None);
    }

    #[test]
    fn the_closest_of_several_plausible_hits_wins() {
        let hits = vec![("near".to_string(), 205.0), ("nearer".to_string(), 211.0)];
        assert_eq!(pick_by_duration(&hits, 210.0).as_deref(), Some("nearer"));
    }

    #[test]
    fn parses_what_yt_dlp_prints_including_its_unknowns() {
        let out = "abc123|212.0\nDEF456|NA\n\nghi789|30\n";
        assert_eq!(
            parse_candidates(out),
            vec![
                ("abc123".to_string(), 212.0),
                ("DEF456".to_string(), 0.0),
                ("ghi789".to_string(), 30.0),
            ]
        );
    }

    #[test]
    fn one_shared_artist_is_the_album_artist_and_a_mix_is_various() {
        let one = playlist(vec![track("1", &["Boards of Canada"]), track("2", &["Boards of Canada", "Guest"])]);
        assert_eq!(album_artist_of(&one), "Boards of Canada");

        let many = playlist(vec![track("1", &["A"]), track("2", &["B"])]);
        assert_eq!(album_artist_of(&many), "Various Artists");

        assert_eq!(album_artist_of(&playlist(vec![])), "");
    }

    #[test]
    fn tolerance_grows_with_the_track() {
        // Twenty seconds is the floor, so a short track is not judged by a
        // percentage of almost nothing.
        assert_eq!(tolerance_s(60.0), 20.0);
        assert!((tolerance_s(600.0) - 48.0).abs() < 1e-9);
    }

    #[test]
    #[ignore] // hits the network; run manually with --ignored
    fn resolve_and_download_one() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::new();
            let pl = crate::resolve_url(
                &client,
                "https://open.spotify.com/track/1eJdXVLxLoMWu1TkaeSL18",
            )
            .await
            .unwrap();
            assert!(!pl.tracks.is_empty());
            let dir = std::env::temp_dir().join("mdl-e2e");
            let opts = DownloadOptions { dest_dir: dir, parallelism: 1, threads_per_download: 2, format: "opus".into(), bitrate: 128, name_method: crate::types::NameMethod::Numbered };
            let cancel = Arc::new(AtomicBool::new(false));
            let sum = download_playlist(&client, &pl, &opts, cancel, |_| {}).await;
            assert!(sum.downloaded + sum.skipped >= 1);
        });
    }
}
