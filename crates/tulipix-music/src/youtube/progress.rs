//! yt-dlp's `--newline` output, one line at a time, as something a job row can
//! show: how far, how big, how fast, how long, and which stage it is in.
//!
//! Line shapes as yt-dlp 2026.08.19 writes them:
//! `[download]   6.4% of  109.96KiB at    4.84MiB/s ETA 00:00`
//! `[download]  62.0% of ~ 99.60MiB at  2.40MiB/s ETA 00:14`
//! `[download]   0.9% of  109.96KiB at  Unknown B/s ETA Unknown`
//! `[download] 100% of  109.96KiB in 00:00:00 at 602.29KiB/s`
//! `[ExtractAudio] Destination: …`, `[Merger] Merging formats into …`, and so on.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Downloading,
    Merging,
    Converting,
    EmbeddingCover,
    WritingTags,
    CuttingSponsors,
}

impl Stage {
    pub fn label(&self) -> &'static str {
        match self {
            Stage::Downloading => "Downloading",
            Stage::Merging => "Merging picture and sound",
            Stage::Converting => "Converting",
            Stage::EmbeddingCover => "Embedding cover",
            Stage::WritingTags => "Writing tags",
            Stage::CuttingSponsors => "Cutting sponsor segments",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub frac: Option<f64>,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
    pub eta_s: Option<u64>,
    pub stage: Option<Stage>,
}

/// `109.96KiB` → bytes. yt-dlp writes binary units; `B` alone is bytes.
fn parse_size(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches('~');
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.trim().parse().ok()?;
    let mult = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(n * mult)
}

/// `00:14` / `1:02:03` → seconds.
fn parse_clock(s: &str) -> Option<u64> {
    s.split(':').try_fold(0u64, |acc, part| Some(acc * 60 + part.parse::<u64>().ok()?))
}

fn stage_of(tag: &str) -> Option<Stage> {
    Some(match tag {
        "download" => Stage::Downloading,
        "Merger" => Stage::Merging,
        "ExtractAudio" | "FixupM4a" | "VideoRemuxer" | "VideoConvertor" => Stage::Converting,
        "EmbedThumbnail" | "ThumbnailsConvertor" => Stage::EmbeddingCover,
        "Metadata" => Stage::WritingTags,
        "SponsorBlock" | "ModifyChapters" => Stage::CuttingSponsors,
        _ => return None,
    })
}

/// One line. `None` for anything that says nothing a job row can use.
pub fn parse_line(line: &str) -> Option<Tick> {
    let line = line.trim();
    let rest = line.strip_prefix('[')?;
    let (tag, body) = rest.split_once(']')?;
    let stage = stage_of(tag)?;
    if stage != Stage::Downloading {
        return Some(Tick { stage: Some(stage), ..Tick::default() });
    }

    let words: Vec<&str> = body.split_whitespace().collect();
    let Some(pct) = words.first().and_then(|w| w.strip_suffix('%')) else {
        // `[download] Destination: …` starts a stream: a stage, no numbers.
        return words
            .first()
            .is_some_and(|w| *w == "Destination:")
            .then(|| Tick { stage: Some(Stage::Downloading), ..Tick::default() });
    };
    let frac = pct.parse::<f64>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))?;
    // The value after a keyword; `of ~ 99.60MiB` puts the tilde in its own word.
    let after = |key: &str| {
        let at = words.iter().position(|w| *w == key)?;
        words[at + 1..].iter().find(|w| **w != "~").copied()
    };
    Some(Tick {
        frac: Some(frac),
        total_bytes: after("of").and_then(parse_size).map(|b| b as u64),
        speed_bps: after("at").and_then(|s| parse_size(s.strip_suffix("/s")?)),
        eta_s: after("ETA").and_then(parse_clock),
        stage: Some(Stage::Downloading),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_line_has_all_four_numbers() {
        let t = parse_line("[download]   6.4% of  109.96KiB at    4.84MiB/s ETA 00:00").unwrap();
        assert!((t.frac.unwrap() - 0.064).abs() < 1e-9);
        assert_eq!(t.total_bytes, Some((109.96 * 1024.0) as u64));
        assert!((t.speed_bps.unwrap() - 4.84 * 1024.0 * 1024.0).abs() < 1.0);
        assert_eq!(t.eta_s, Some(0));
        assert_eq!(t.stage, Some(Stage::Downloading));
    }

    #[test]
    fn approximate_sizes_and_long_etas() {
        let t = parse_line("[download]  62.0% of ~ 99.60MiB at  2.40MiB/s ETA 1:02:03").unwrap();
        assert_eq!(t.total_bytes, Some((99.60 * 1024.0 * 1024.0) as u64));
        assert_eq!(t.eta_s, Some(3723));
        let glued = parse_line("[download]   0.0% of ~1.00MiB at 1.00KiB/s ETA 00:10").unwrap();
        assert_eq!(glued.total_bytes, Some(1024 * 1024));
    }

    #[test]
    fn unknown_speed_and_eta_are_none_not_zero() {
        let t = parse_line("[download]   0.9% of  109.96KiB at  Unknown B/s ETA Unknown").unwrap();
        assert!(t.frac.is_some() && t.total_bytes.is_some());
        assert_eq!((t.speed_bps, t.eta_s), (None, None));
    }

    #[test]
    fn the_finished_line_is_one_hundred_percent() {
        let t = parse_line("[download] 100% of  109.96KiB in 00:00:00 at 602.29KiB/s").unwrap();
        assert_eq!(t.frac, Some(1.0));
        assert_eq!(t.eta_s, None);
    }

    #[test]
    fn post_processors_are_stages() {
        let stage = |l: &str| parse_line(l).and_then(|t| t.stage);
        assert_eq!(stage("[Merger] Merging formats into \"x.mkv\""), Some(Stage::Merging));
        assert_eq!(stage("[ExtractAudio] Destination: /tmp/x.mp3"), Some(Stage::Converting));
        assert_eq!(stage("[EmbedThumbnail] mutagen: Adding thumbnail to \"x.opus\""), Some(Stage::EmbeddingCover));
        assert_eq!(stage("[ThumbnailsConvertor] Converting thumbnail \"x.webp\" to jpg"), Some(Stage::EmbeddingCover));
        assert_eq!(stage("[Metadata] Adding metadata to \"x.mp3\""), Some(Stage::WritingTags));
        assert_eq!(stage("[SponsorBlock] Found 2 segments in the SponsorBlock database"), Some(Stage::CuttingSponsors));
        assert_eq!(stage("[download] Destination: /tmp/x.webm"), Some(Stage::Downloading));
        assert_eq!(parse_line("[download] Destination: /tmp/x.webm").unwrap().frac, None);
    }

    #[test]
    fn everything_else_is_none() {
        assert_eq!(parse_line("[youtube] abc123: Downloading webpage"), None);
        assert_eq!(parse_line("[info] Writing '%(filepath)s' to: /tmp/p.txt"), None);
        assert_eq!(parse_line("Deleting original file /tmp/x.webm (pass -k to keep)"), None);
        assert_eq!(parse_line("/home/me/Music/YouTube/x.mp3"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("[download] has already been downloaded"), None);
    }
}
