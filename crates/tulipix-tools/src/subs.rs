//! Subtitle surgery — retiming and tidying, on the file rather than through
//! ffmpeg.
//!
//! `preview::parse_cues` reads subtitles too, and deliberately differently:
//! it flattens a cue to one line of plain text for display and throws away
//! everything it does not draw. This one round-trips. A cue that goes in with
//! two lines and an italic tag comes out with two lines and an italic tag,
//! unless the caller asked for the tag to go.
//!
//! Timestamps are milliseconds as `i64`, signed on purpose: shifting a
//! subtitle two seconds earlier takes the first cue negative before it is
//! clamped, and doing that in `u64` wraps to a cue that starts in the year
//! 584 million.

/// One subtitle, with its text exactly as it appeared.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

impl Line {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }
}

/// Read SRT or WebVTT. The two differ in a header, a decimal separator and
/// some cue settings, none of which change how the file is walked.
pub fn parse(body: &str) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    // A BOM on the first line stops the first timestamp matching, which shows
    // up as "the first subtitle is missing" and nothing else.
    let body = body.trim_start_matches('\u{feff}').replace("\r\n", "\n");

    for block in body.split("\n\n") {
        let block = block.trim_matches('\n');
        if block.is_empty() {
            continue;
        }
        let mut lines = block.lines().peekable();
        // An SRT block opens with a sequence number, a VTT block may open with
        // a cue identifier, and either may open with the timing itself.
        let mut timing = lines.next().unwrap_or_default();
        if !timing.contains("-->") {
            match lines.next() {
                Some(next) if next.contains("-->") => timing = next,
                _ => continue,
            }
        }
        let Some((start_ms, end_ms)) = split_timing(timing) else {
            continue;
        };
        let text = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        out.push(Line {
            start_ms,
            end_ms,
            text,
        });
    }
    out
}

fn split_timing(line: &str) -> Option<(i64, i64)> {
    let (a, b) = line.split_once("-->")?;
    // VTT puts cue settings after the end time: `... --> 00:03.500 line:90%`.
    let b = b.split_whitespace().next()?;
    Some((parse_stamp(a.trim())?, parse_stamp(b.trim())?))
}

/// `01:02:03,456`, `01:02:03.456` and the VTT short form `02:03.456`.
pub fn parse_stamp(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    let (clock, frac) = match s.split_once('.') {
        Some((c, f)) => (c, f),
        None => (s.as_str(), "0"),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<i64>().ok()?,
            m.parse::<i64>().ok()?,
            s.parse::<i64>().ok()?,
        ),
        [m, s] => (0, m.parse::<i64>().ok()?, s.parse::<i64>().ok()?),
        _ => return None,
    };
    // Milliseconds, whatever precision was written: "5" is 500 ms, not 5.
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

/// `01:02:03,456` — the SRT spelling.
pub fn stamp(ms: i64) -> String {
    let ms = ms.max(0);
    let (h, rest) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rest) = (rest / 60_000, rest % 60_000);
    format!("{h:02}:{m:02}:{:02},{:03}", rest / 1000, rest % 1000)
}

pub fn to_srt(lines: &[Line]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            stamp(line.start_ms),
            stamp(line.end_ms),
            line.text
        ));
    }
    out
}

pub fn to_vtt(lines: &[Line]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for line in lines {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            stamp(line.start_ms).replace(',', "."),
            stamp(line.end_ms).replace(',', "."),
            line.text
        ));
    }
    out
}

/// Move every cue by `by_ms`, and stretch by `rate` while doing it.
///
/// `rate` is for the one bug that a shift cannot fix: a subtitle authored
/// against 25 fps played back at 23.976 drifts further out the longer it runs,
/// and no single offset lines up both the first line and the last. 23.976/25
/// is 0.95904, and that is the number to type.
///
/// A cue dragged before zero is clamped rather than dropped: a line that
/// belongs at −0.4 s belongs at the start of the film, and deleting it loses
/// a subtitle to fix a timing.
pub fn retime(lines: &[Line], by_ms: i64, rate: f64) -> Vec<Line> {
    let rate = if rate > 0.0 { rate } else { 1.0 };
    lines
        .iter()
        .map(|l| {
            let start = (l.start_ms as f64 * rate) as i64 + by_ms;
            let end = (l.end_ms as f64 * rate) as i64 + by_ms;
            Line {
                start_ms: start.max(0),
                // Clamping the start must not collapse the cue onto itself.
                end_ms: end.max(start.max(0)),
                text: l.text.clone(),
            }
        })
        .collect()
}

/// What "clean up" does. Every one of these is off unless asked for: a
/// subtitle file is someone's work, and a tidy-up that silently rewrote the
/// text would be worse than the overlaps it fixed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tidy {
    /// Remove `<i>`, `<b>`, `<font …>` and ASS `{\pos(…)}` override blocks.
    pub strip_tags: bool,
    /// Pull an end time back so it does not run past the next start.
    pub fix_overlaps: bool,
    /// Drop cues with no text left.
    pub drop_empty: bool,
    /// Push an end time out so no cue is on screen for less than this.
    /// Zero leaves durations alone.
    pub min_ms: i64,
}

pub fn tidy(lines: &[Line], opts: Tidy) -> Vec<Line> {
    let mut out: Vec<Line> = lines
        .iter()
        .map(|l| Line {
            text: if opts.strip_tags {
                strip_markup(&l.text)
            } else {
                l.text.clone()
            },
            ..l.clone()
        })
        .collect();

    if opts.drop_empty {
        out.retain(|l| !l.text.trim().is_empty());
    }
    if opts.min_ms > 0 {
        for l in out.iter_mut() {
            if l.duration_ms() < opts.min_ms {
                l.end_ms = l.start_ms + opts.min_ms;
            }
        }
    }
    if opts.fix_overlaps {
        // Backwards, so extending a cue to the minimum duration above cannot
        // push it over a neighbour this pass has already checked.
        for i in (0..out.len().saturating_sub(1)).rev() {
            let next_start = out[i + 1].start_ms;
            if out[i].end_ms > next_start {
                // One frame at 24 fps, so two cues never share an instant.
                out[i].end_ms = (next_start - 42).max(out[i].start_ms);
            }
        }
    }
    out
}

/// HTML-ish tags and ASS override blocks. Deliberately not a parser: subtitle
/// markup is not nested and never has been.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' | '{' => depth += 1,
            '>' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    // ASS writes its line break as a literal backslash-N.
    out.replace("\\N", "\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:03,500\nHello\nthere\n\n2\n00:00:04,000 --> 00:00:05,000\n<i>Bye</i>\n";

    #[test]
    fn a_cue_keeps_its_line_breaks_and_its_markup() {
        let lines = parse(SRT);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello\nthere");
        assert_eq!(lines[0].start_ms, 1000);
        assert_eq!(lines[0].end_ms, 3500);
        assert_eq!(lines[1].text, "<i>Bye</i>");
    }

    #[test]
    fn vtt_short_stamps_and_cue_settings_both_parse() {
        let lines = parse("WEBVTT\n\n00:01.000 --> 00:03.500 line:90%\nHello\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start_ms, 1000);
        assert_eq!(lines[0].end_ms, 3500);
    }

    #[test]
    fn srt_round_trips_through_the_writer() {
        assert_eq!(parse(&to_srt(&parse(SRT))), parse(SRT));
    }

    #[test]
    fn shifting_earlier_clamps_at_zero_rather_than_wrapping() {
        let lines = retime(&parse(SRT), -2000, 1.0);
        assert_eq!(lines[0].start_ms, 0);
        // The clamp must not leave the cue ending before it starts.
        assert!(lines[0].end_ms >= lines[0].start_ms);
        assert_eq!(lines[0].end_ms, 1500);
        assert_eq!(lines[1].start_ms, 2000);
    }

    #[test]
    fn a_framerate_fix_drifts_the_way_the_film_does() {
        // 25 fps authored, 23.976 played: every cue lands slightly earlier,
        // and the later the cue the bigger the correction.
        let lines = retime(&parse(SRT), 0, 23.976 / 25.0);
        assert_eq!(lines[0].start_ms, 959);
        assert_eq!(lines[1].start_ms, 3836);
    }

    #[test]
    fn tidying_strips_markup_and_pulls_overlaps_apart() {
        let overlapping = vec![
            Line {
                start_ms: 0,
                end_ms: 5000,
                text: "<i>one</i>".into(),
            },
            Line {
                start_ms: 2000,
                end_ms: 3000,
                text: "{\\pos(1,2)}two".into(),
            },
            Line {
                start_ms: 4000,
                end_ms: 4100,
                text: "   ".into(),
            },
        ];
        let out = tidy(
            &overlapping,
            Tidy {
                strip_tags: true,
                fix_overlaps: true,
                drop_empty: true,
                min_ms: 0,
            },
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "one");
        assert_eq!(out[1].text, "two");
        assert!(out[0].end_ms < out[1].start_ms);
    }

    #[test]
    fn a_minimum_duration_never_swallows_the_next_cue() {
        let tight = vec![
            Line {
                start_ms: 0,
                end_ms: 100,
                text: "a".into(),
            },
            Line {
                start_ms: 500,
                end_ms: 2000,
                text: "b".into(),
            },
        ];
        let out = tidy(
            &tight,
            Tidy {
                fix_overlaps: true,
                min_ms: 1000,
                ..Default::default()
            },
        );
        // Asked for a full second, given what the next cue leaves.
        assert!(out[0].end_ms < out[1].start_ms);
        assert!(out[0].end_ms > 100);
    }

    #[test]
    fn stamps_survive_the_round_trip() {
        assert_eq!(stamp(3_723_456), "01:02:03,456");
        assert_eq!(parse_stamp("01:02:03,456"), Some(3_723_456));
        assert_eq!(parse_stamp("02:03.5"), Some(123_500));
        assert_eq!(stamp(-5), "00:00:00,000");
    }
}
