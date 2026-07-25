//! Read-aloud text prep. Splits book text into sentences for the read-along
//! highlighter (Phase 1) and, later, for Kokoro TTS synthesis (Phase 2). Pure
//! functions — no engine here yet.

/// One spoken unit: the sentence text plus its char offset in the source, so a
/// click on a sentence can map back to a reading position.
#[derive(Debug, Clone)]
pub struct Sentence {
    pub text: String,
    pub char_start: usize,
}

/// Split `text` into sentences. Breaks after `.`, `!`, `?` (and `…`) when
/// followed by whitespace, with a light guard against common abbreviations and
/// decimals so "Mr. Smith" / "3.5" don't split. Paragraph breaks always split.
pub fn sentences(text: &str) -> Vec<Sentence> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize; // char index of the current sentence start
    let mut i = 0usize;
    let n = chars.len();

    let flush = |out: &mut Vec<Sentence>, s: usize, e: usize| {
        // `text` is trimmed, so char_start has to skip the same leading
        // whitespace. Reporting the pre-trim index made the two fields disagree:
        // a sentence after "One. " began at the space, so read-aloud highlighting
        // started a character early — and more than one after a paragraph break.
        let mut s = s;
        while s < e && chars[s].is_whitespace() {
            s += 1;
        }
        let t: String = chars_range(text, s, e).trim().to_string();
        if !t.is_empty() {
            out.push(Sentence { text: t, char_start: s });
        }
    };

    while i < n {
        let c = chars[i];
        let is_end = matches!(c, '.' | '!' | '?' | '…');
        // Two consecutive newlines → paragraph boundary.
        let para_break = c == '\n' && chars.get(i + 1) == Some(&'\n');
        if is_end {
            // Skip decimals like "3.14" and single-letter abbreviations.
            let prev = if i > 0 { chars[i - 1] } else { ' ' };
            let next = chars.get(i + 1).copied().unwrap_or(' ');
            let decimal = c == '.' && prev.is_ascii_digit() && next.is_ascii_digit();
            let abbrev = c == '.' && is_abbrev(&chars, start, i);
            if !decimal && !abbrev {
                // Consume trailing quotes/brackets, then require whitespace/eof.
                let mut j = i + 1;
                while j < n && matches!(chars[j], '"' | '\'' | '”' | '’' | ')' | ']') {
                    j += 1;
                }
                if j >= n || chars[j].is_whitespace() {
                    flush(&mut out, start, j);
                    start = j;
                    i = j;
                    continue;
                }
            }
        } else if para_break {
            flush(&mut out, start, i);
            start = i + 1;
        }
        i += 1;
    }
    flush(&mut out, start, n);
    out
}

/// True when the token ending at `end` (exclusive of the '.') is a known
/// abbreviation ("Mr", "Dr", "St", "vs", …) so we don't split after it.
fn is_abbrev(chars: &[char], start: usize, end: usize) -> bool {
    // Grab the alphabetic run immediately before the dot.
    let mut b = end;
    while b > start && chars[b - 1].is_alphabetic() {
        b -= 1;
    }
    let word: String = chars[b..end].iter().collect::<String>().to_lowercase();
    // Single letter (initials) or common title/abbr.
    word.chars().count() == 1
        || matches!(
            word.as_str(),
            "mr" | "mrs" | "ms" | "dr" | "st" | "vs" | "etc" | "jr" | "sr" | "prof" | "inc" | "ltd" | "no" | "vol"
        )
}

/// Char-index substring (chars, not bytes) of `s`.
fn chars_range(s: &str, start: usize, end: usize) -> String {
    s.chars().skip(start).take(end.saturating_sub(start)).collect()
}

/// Rough spoken duration (ms) for a sentence at `wpm` words/min and `speed`
/// multiplier — used to pace the read-along highlight before real audio.
pub fn speak_ms(sentence: &str, wpm: f32, speed: f32) -> u64 {
    let words = sentence.split_whitespace().count().max(1) as f32;
    let base = words / (wpm.max(60.0) * speed.max(0.25)) * 60_000.0;
    // Small floor so very short lines still get a beat.
    (base as u64).max(350)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_basic() {
        let s = sentences("Hello world. This is a test! Really? Yes.");
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].text, "Hello world.");
        assert_eq!(s[2].text, "Really?");
    }

    #[test]
    fn keeps_abbrev_and_decimal() {
        let s = sentences("Mr. Smith paid 3.5 dollars. Done.");
        assert_eq!(s.len(), 2);
        assert!(s[0].text.starts_with("Mr. Smith"));
    }

    #[test]
    fn char_start_maps_back() {
        let s = sentences("One. Two.");
        assert_eq!(s[1].char_start, 5);
    }
}
