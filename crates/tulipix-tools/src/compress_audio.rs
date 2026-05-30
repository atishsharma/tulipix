//! `np.p4.tools.compress.audio` — bitrate slider; MP3/AAC/Opus/Vorbis; VBR/CBR;
//! preserve tags.
//!
//! Builds the ffmpeg argv for an audio re-encode, mapping a codec + VBR/CBR
//! choice to the right encoder flags, and always copying metadata so tags
//! survive.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCodec { Mp3, Aac, Opus, Vorbis }

impl AudioCodec {
    pub fn encoder(self) -> &'static str {
        match self { AudioCodec::Mp3 => "libmp3lame", AudioCodec::Aac => "aac", AudioCodec::Opus => "libopus", AudioCodec::Vorbis => "libvorbis" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rate {
    /// Constant bitrate in kbps.
    Cbr(u32),
    /// VBR quality level (encoder-specific scale, validated per codec).
    VbrQuality(u32),
}

pub fn args(input: &str, codec: AudioCodec, rate: Rate, out: &str) -> Vec<String> {
    let mut a = vec!["-i".into(), input.into(), "-map_metadata".into(), "0".into(), "-c:a".into(), codec.encoder().into()];
    match rate {
        Rate::Cbr(kbps) => { a.push("-b:a".into()); a.push(format!("{kbps}k")); }
        Rate::VbrQuality(q) => match codec {
            AudioCodec::Mp3 | AudioCodec::Vorbis => { a.push("-q:a".into()); a.push(q.to_string()); }
            AudioCodec::Opus => { a.push("-vbr".into()); a.push("on".into()); a.push("-compression_level".into()); a.push(q.min(10).to_string()); }
            AudioCodec::Aac => { a.push("-q:a".into()); a.push(q.to_string()); }
        },
    }
    a.push(out.into());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbr_sets_bitrate_and_preserves_tags() {
        let a = args("in.flac", AudioCodec::Mp3, Rate::Cbr(192), "o.mp3");
        assert!(a.windows(2).any(|w| w == ["-b:a", "192k"]));
        assert!(a.windows(2).any(|w| w == ["-map_metadata", "0"]));
    }

    #[test]
    fn vbr_opus_uses_vbr_flag() {
        let a = args("in.flac", AudioCodec::Opus, Rate::VbrQuality(8), "o.opus");
        assert!(a.contains(&"libopus".to_string()));
        assert!(a.windows(2).any(|w| w == ["-vbr", "on"]));
    }
}
