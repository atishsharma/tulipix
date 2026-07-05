//! Download pipeline — port of mdl `sync.ts` + `youtube.ts` + `metadata.ts`.
//!
//! Per track: yt-dlp searches YouTube and extracts Opus, then lofty writes the
//! provider metadata (title/artist/album + cover art) over yt-dlp's guess.

use crate::manifest::{self, Manifest, ManifestTrack};
use crate::types::{DownloadOptions, Playlist, Progress, Stage, Summary, Track};
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// `"NN - Artist, Artist - Title"`, filesystem-safe.
pub fn track_file_stem(index: usize, track: &Track) -> String {
    let raw = format!("{:02} - {} - {}", index, track.artists.join(", "), track.title);
    sanitize_filename::sanitize(raw)
}

fn search_query(track: &Track) -> String {
    format!("{} - {}", track.artists.join(", "), track.title)
}

fn ytdlp_bin() -> PathBuf {
    tulipix_core::thumbs::tool_bin("yt-dlp")
}

/// Download one track's audio into `dest` as Opus. Returns the produced path.
async fn download_audio(track: &Track, index: usize, dest: &Path) -> Result<PathBuf> {
    let stem = track_file_stem(index, track);
    let out_tmpl = dest.join(format!("{stem}.%(ext)s"));
    let query = format!("ytsearch1:{}", search_query(track));
    let out = tokio::process::Command::new(ytdlp_bin())
        .arg(&query)
        .args(["-f", "bestaudio", "-x", "--audio-format", "opus"])
        .args(["--no-playlist", "--no-warnings", "-o"])
        .arg(&out_tmpl)
        .output()
        .await?;
    if !out.status.success() {
        bail!("yt-dlp failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let path = dest.join(format!("{stem}.opus"));
    if !path.exists() {
        bail!("expected output file missing: {}", path.display());
    }
    Ok(path)
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
            let pic = Picture::new_unchecked(
                PictureType::CoverFront,
                Some(MimeType::Jpeg),
                None,
                bytes,
            );
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

    let manifest = Arc::new(Mutex::new(manifest::load(&opts.dest_dir).unwrap_or_else(
        || Manifest::new(playlist.provider, &playlist.id, &playlist.title, &playlist.source_url),
    )));
    let total = playlist.tracks.len();
    let sem = Arc::new(Semaphore::new(opts.parallelism.max(1)));
    // (downloaded, skipped, failed)
    let counters = Arc::new(Mutex::new((0usize, 0usize, Vec::<(Track, String)>::new())));

    let mut handles = Vec::new();
    for (i, track) in playlist.tracks.iter().cloned().enumerate() {
        let sem = sem.clone();
        let client = client.clone();
        let dest = opts.dest_dir.clone();
        let cancel = cancel.clone();
        let on_progress = on_progress.clone();
        let counters = counters.clone();
        let manifest = manifest.clone();

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
            match download_audio(&track, idx, &dest).await {
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
            album: None,
            artwork_url: None,
            duration_ms: None,
            source_url: None,
        };
        assert_eq!(track_file_stem(3, &t), "03 - A, B - SongName");
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
            let opts = DownloadOptions { dest_dir: dir, parallelism: 1 };
            let cancel = Arc::new(AtomicBool::new(false));
            let sum = download_playlist(&client, &pl, &opts, cancel, |_| {}).await;
            assert!(sum.downloaded + sum.skipped >= 1);
        });
    }
}
