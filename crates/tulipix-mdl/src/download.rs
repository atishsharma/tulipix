//! Download pipeline — port of mdl `sync.ts` + `youtube.ts` + `metadata.ts`.
//!
//! Per track: yt-dlp searches YouTube and extracts Opus, then lofty writes the
//! provider metadata (title/artist/album + cover art) over yt-dlp's guess.

use crate::manifest::{self, Manifest, ManifestTrack};
use crate::types::{DownloadOptions, NameMethod, Playlist, Progress, Stage, Summary, Track};
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
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
    tulipix_core::thumbs::tool_bin("yt-dlp")
}

/// yt-dlp cookie flags from the stored `ytdlp_cookies` setting: a cookies.txt
/// path -> `--cookies <path>`, otherwise a browser name -> `--cookies-from-browser`.
fn cookie_args() -> Vec<String> {
    match tulipix_core::api_keys::fetch("ytdlp_cookies") {
        Ok(Some(v)) if !v.trim().is_empty() => {
            let v = v.trim().to_string();
            if std::path::Path::new(&v).is_file() {
                vec!["--cookies".into(), v]
            } else {
                vec!["--cookies-from-browser".into(), v]
            }
        }
        _ => Vec::new(),
    }
}

/// Download one track's audio into `dest` as Opus. Returns the produced path.
/// Searches YouTube for the single best match of "artist - title" (per track,
/// never the source playlist) and extracts audio to Opus via ffmpeg.
/// How many times to (re)try a single track before giving up. YouTube throws
/// transient 403/network hiccups; a couple of retries clears most of them.
const MAX_TRIES: usize = 3;

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
    let query = format!("ytsearch1:{}", search_query(track));
    let ffmpeg = tulipix_core::thumbs::tool_bin("ffmpeg");
    let path = dest.join(format!("{stem}.{format}"));

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
        cmd
            // Threads per download — parallel fragment downloads for this track.
            .args(["--concurrent-fragments", &threads.max(1).to_string()])
            // Rotate player clients — helps dodge YouTube's "confirm you're not a
            // bot" gate that hits the default web client.
            .args(["--extractor-args", "youtube:player_client=default,tv,android"]);
        // yt-dlp needs ffmpeg for the Opus extraction. Point it at the resolved
        // binary so it works even when ffmpeg isn't on the app process's PATH.
        if ffmpeg.exists() {
            cmd.arg("--ffmpeg-location").arg(&ffmpeg);
        }
        // Optional cookies (Settings → "yt-dlp cookies"): a cookies.txt path or a
        // browser name for --cookies-from-browser. The only reliable way past
        // YouTube's bot check.
        for a in cookie_args() {
            cmd.arg(a);
        }
        cmd.arg("-o").arg(&out_tmpl);

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
                // The bot check never clears by retrying — fail fast.
                if stderr.contains("confirm you") || stderr.contains("not a bot") || stderr.contains("Sign in") {
                    bail!("YouTube bot check — set browser cookies in Settings → yt-dlp cookies");
                }
                last_err = anyhow!("yt-dlp: {}", stderr.lines().last().unwrap_or("failed").trim().to_string());
            }
        }
        if attempt < MAX_TRIES {
            tokio::time::sleep(std::time::Duration::from_millis(800 * attempt as u64)).await;
        }
    }
    Err(last_err)
}

/// Overwrite tags + cover art with the provider metadata via lofty.
async fn write_tags(path: &Path, track: &Track, client: &reqwest::Client) -> Result<()> {
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

    tokio::task::spawn_blocking(move || -> Result<()> {
        use lofty::config::WriteOptions;
        use lofty::file::TaggedFileExt;
        use lofty::picture::{MimeType, Picture, PictureType};
        use lofty::prelude::{Accessor, TagExt};
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
            let pic = Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, bytes);
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
                    if let Err(e) = write_tags(&path, &track, &client).await {
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
