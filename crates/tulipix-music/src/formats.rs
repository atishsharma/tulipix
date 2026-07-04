//! `np.p4.music.formats` — format support audit.
//!
//! Single source of truth for which audio containers Tulipix claims to play
//! and whether gapless is verified for each. The Settings → "Format support"
//! table and the scanner's accept-filter both read this.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatInfo {
    pub ext: &'static str,
    pub lossless: bool,
    pub gapless_verified: bool,
    pub note: &'static str,
}

pub const FORMATS: &[FormatInfo] = &[
    FormatInfo { ext: "flac", lossless: true,  gapless_verified: true,  note: "native" },
    FormatInfo { ext: "alac", lossless: true,  gapless_verified: true,  note: "in m4a" },
    FormatInfo { ext: "m4a",  lossless: false, gapless_verified: true,  note: "AAC/ALAC" },
    FormatInfo { ext: "aac",  lossless: false, gapless_verified: false, note: "raw ADTS, gapless not guaranteed" },
    FormatInfo { ext: "mp3",  lossless: false, gapless_verified: true,  note: "LAME gapless info honored" },
    FormatInfo { ext: "opus", lossless: false, gapless_verified: true,  note: "native" },
    FormatInfo { ext: "ogg",  lossless: false, gapless_verified: true,  note: "Vorbis" },
    FormatInfo { ext: "wav",  lossless: true,  gapless_verified: true,  note: "PCM" },
    FormatInfo { ext: "aiff", lossless: true,  gapless_verified: true,  note: "PCM" },
    FormatInfo { ext: "dsf",  lossless: true,  gapless_verified: false, note: "DSD via DoP" },
    FormatInfo { ext: "dff",  lossless: true,  gapless_verified: false, note: "DSD via DoP" },
    FormatInfo { ext: "wv",   lossless: true,  gapless_verified: false, note: "WavPack" },
    FormatInfo { ext: "ape",  lossless: true,  gapless_verified: false, note: "Monkey's Audio" },
    FormatInfo { ext: "wma",  lossless: false, gapless_verified: false, note: "legacy Windows Media" },
    FormatInfo { ext: "mka",  lossless: false, gapless_verified: false, note: "Matroska audio, codec varies" },
    FormatInfo { ext: "mpc",  lossless: false, gapless_verified: false, note: "Musepack" },
    FormatInfo { ext: "tta",  lossless: true,  gapless_verified: false, note: "True Audio" },
];

pub fn lookup(ext: &str) -> Option<&'static FormatInfo> {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    FORMATS.iter().find(|f| f.ext == e)
}

pub fn is_supported(ext: &str) -> bool { lookup(ext).is_some() }

/// Extensions still pending gapless verification — the audit's TODO list.
pub fn gapless_gaps() -> Vec<&'static str> {
    FORMATS.iter().filter(|f| !f.gapless_verified).map(|f| f.ext).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_and_dot_insensitive() {
        assert!(is_supported(".FLAC"));
        assert!(is_supported("opus"));
        assert!(!is_supported("xyz"));
        assert!(lookup("dsf").unwrap().lossless);
    }

    #[test]
    fn gapless_audit_flags_dsd_and_aac() {
        let gaps = gapless_gaps();
        assert!(gaps.contains(&"dsf"));
        assert!(gaps.contains(&"aac"));
        assert!(!gaps.contains(&"flac"));
    }
}
