//! Kokoro-82M neural TTS for read-aloud (Phase 2).
//!
//! Pipeline: text → espeak-ng IPA phonemes → Kokoro token ids → ONNX
//! (kokoro-82M) → 24 kHz mono waveform. The reader plays each sentence's
//! waveform through mpv and advances the highlight on audio-end.
//!
//! Gated behind the `kokoro` feature (pulls `ort`). When the feature is off, or
//! at runtime when the model / espeak-ng / voice files are missing,
//! [`available`] returns false and the reader falls back to the timer-paced
//! highlight — nothing breaks without the model.
//!
//! On-disk layout (populated by the AI-models download of `kokoro-82m`):
//!   <data_dir>/models/kokoro-82m-<ver>/
//!       kokoro-82m.onnx            ← the model (manifest download)
//!       voices/<voice_id>.bin      ← per-voice style, raw little-endian f32,
//!                                     shape [510, 256] (row = token count)

/// Kokoro output sample rate.
pub const SAMPLE_RATE: u32 = 24_000;

/// Locate the installed kokoro model directory (versioned; prefix scan so a
/// manifest version bump doesn't strand it). Returns the dir, not the file.
pub fn model_dir() -> Option<std::path::PathBuf> {
    let root = tulipix_core::paths::data_dir()?.join("models");
    let rd = std::fs::read_dir(&root).ok()?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("kokoro-82m-") && e.path().is_dir() {
            return Some(e.path());
        }
    }
    None
}

/// The model `.onnx` inside the kokoro dir (first `*.onnx` found).
fn model_file() -> Option<std::path::PathBuf> {
    let dir = model_dir()?;
    std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("onnx")).unwrap_or(false)
    })
}

/// Path to a voice's style file, if present.
fn voice_file(voice: &str) -> Option<std::path::PathBuf> {
    let p = model_dir()?.join("voices").join(format!("{voice}.bin"));
    p.exists().then_some(p)
}

/// True when a real Kokoro synth can run for `voice`: model + espeak-ng + the
/// voice's style file are all present. Always false without the `kokoro` feature.
pub fn available(voice: &str) -> bool {
    cfg!(feature = "kokoro") && model_file().is_some() && voice_file(voice).is_some() && espeak_ok()
}

/// Is espeak-ng runnable (phonemizer backend)? Uses the shared resolver so a
/// bundled espeak-ng (resources/bin) counts, not just a PATH install.
fn espeak_ok() -> bool {
    crate::speech::espeak_available()
}

// ── phonemes + tokens ─────────────────────────────────────────────────────────
// Only compiled with the `kokoro` feature (or under test) — no dead-code warning
// in the lite build.

// Kokoro symbol table (matches the model's config.json vocab). Token id = index
// into `$` + punctuation + letters + IPA letters.
#[cfg(any(feature = "kokoro", test))]
const PAD: char = '$';
#[cfg(any(feature = "kokoro", test))]
const PUNCT: &str = ";:,.!?¡¿—…\"«»“” ";
#[cfg(any(feature = "kokoro", test))]
const LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
#[cfg(any(feature = "kokoro", test))]
const LETTERS_IPA: &str =
    "ɑɐɒæɓʙβɔɕçɗɖðʤəɘɚɛɜɝɞɟʄɡɠɢʛɦɧħɥʜɨɪʝɭɬɫɮʟɱɯɰŋɳɲɴøɵɸθœɶʘɹɺɾɻʀʁɽʂʃʈʧʉʊʋⱱʌɣɤʍχʎʏʑʐʒʔʡʕʢǀǁǂǃˈˌːˑʼʴʰʱʲʷˠˤ˞↓↑→↗↘'̩'ᵻ";

/// char → token id map (built once).
#[cfg(any(feature = "kokoro", test))]
fn vocab() -> &'static std::collections::HashMap<char, i64> {
    use std::sync::OnceLock;
    static V: OnceLock<std::collections::HashMap<char, i64>> = OnceLock::new();
    V.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        let mut id: i64 = 0;
        let push = |c: char, m: &mut std::collections::HashMap<char, i64>, id: &mut i64| {
            m.entry(c).or_insert_with(|| {
                let v = *id;
                *id += 1;
                v
            });
        };
        push(PAD, &mut m, &mut id);
        for c in PUNCT.chars() { push(c, &mut m, &mut id); }
        for c in LETTERS.chars() { push(c, &mut m, &mut id); }
        for c in LETTERS_IPA.chars() { push(c, &mut m, &mut id); }
        m
    })
}

/// espeak language for a Kokoro voice id (af/am = US English, bf/bm = British).
/// Feature-only: the tests cover the vocab/token path, not the espeak call, so
/// building this under plain `test` would be dead code.
#[cfg(feature = "kokoro")]
fn voice_lang(voice: &str) -> &'static str {
    if voice.starts_with("bf") || voice.starts_with("bm") { "en-gb" } else { "en-us" }
}

/// Phonemize `text` to an IPA string via espeak-ng. Empty on failure.
#[cfg(feature = "kokoro")]
fn phonemize(text: &str, lang: &str) -> String {
    let out = crate::speech::espeak_command()
        .args(["-q", "--ipa=3", "-v", lang])
        .arg(text)
        .output();
    let Ok(out) = out else { return String::new() };
    if !out.status.success() {
        return String::new();
    }
    // espeak may emit language-switch markers like "(en)"; drop parens content
    // and collapse whitespace/newlines to single spaces.
    let raw = String::from_utf8_lossy(&out.stdout);
    let mut s = String::with_capacity(raw.len());
    let mut skip = false;
    for c in raw.chars() {
        match c {
            '(' => skip = true,
            ')' => skip = false,
            '\n' | '\r' | '\t' => s.push(' '),
            _ if !skip => s.push(c),
            _ => {}
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Phoneme string → Kokoro token ids (unknown chars dropped). Not padded.
#[cfg(any(feature = "kokoro", test))]
fn tokens(phonemes: &str) -> Vec<i64> {
    let v = vocab();
    phonemes.chars().filter_map(|c| v.get(&c).copied()).collect()
}

// ── engine ────────────────────────────────────────────────────────────────────

#[cfg(feature = "kokoro")]
mod engine {
    use super::*;
    use anyhow::{Context, Result};
    use std::sync::{Mutex, OnceLock};

    fn oe(e: ort::Error) -> anyhow::Error {
        anyhow::anyhow!("ort: {e}")
    }

    struct Engine {
        session: ort::session::Session,
        ids_name: String,
        style_name: String,
        speed_name: String,
        out_name: String,
        /// Cached voice style blocks (voice → [510*256] f32).
        voices: std::collections::HashMap<String, Vec<f32>>,
    }

    impl Engine {
        fn load() -> Result<Self> {
            let model = model_file().context("kokoro model not installed")?;
            let session = ort::session::Session::builder()
                .map_err(oe)?
                .with_execution_providers([
                    ort::execution_providers::CPUExecutionProvider::default().build(),
                ])
                .map_err(oe)?
                .commit_from_file(&model)
                .map_err(oe)
                .with_context(|| format!("load kokoro model {}", model.display()))?;
            // Resolve input names: match by substring, fall back to position.
            let names: Vec<String> = session.inputs.iter().map(|i| i.name.clone()).collect();
            let pick = |want: &str, idx: usize| -> String {
                names
                    .iter()
                    .find(|n| n.to_lowercase().contains(want))
                    .cloned()
                    .unwrap_or_else(|| names.get(idx).cloned().unwrap_or_default())
            };
            let ids_name = names
                .iter()
                .find(|n| {
                    let l = n.to_lowercase();
                    l.contains("token") || l.contains("input") || l.contains("ids")
                })
                .cloned()
                .unwrap_or_else(|| names.first().cloned().unwrap_or_default());
            let style_name = pick("style", 1);
            let speed_name = pick("speed", 2);
            let out_name = session.outputs.first().context("kokoro model has no output")?.name.clone();
            Ok(Self {
                session,
                ids_name,
                style_name,
                speed_name,
                out_name,
                voices: std::collections::HashMap::new(),
            })
        }

        /// Style vector for a voice at `token_len` (row-indexed, clamped).
        fn style(&mut self, voice: &str, token_len: usize) -> Result<Vec<f32>> {
            if !self.voices.contains_key(voice) {
                let path = voice_file(voice).context("voice style file missing")?;
                let bytes = std::fs::read(&path)?;
                anyhow::ensure!(bytes.len() % 4 == 0, "voice file not f32-aligned");
                let floats: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                self.voices.insert(voice.to_string(), floats);
            }
            let all = &self.voices[voice];
            let rows = all.len() / 256;
            anyhow::ensure!(rows > 0, "voice style has no rows");
            let row = token_len.min(rows - 1);
            Ok(all[row * 256..row * 256 + 256].to_vec())
        }

        fn run(&mut self, ids: &[i64], voice: &str, speed: f32) -> Result<Vec<f32>> {
            let style = self.style(voice, ids.len())?;
            // Pad with a leading + trailing 0 (Kokoro convention).
            let mut wrapped = Vec::with_capacity(ids.len() + 2);
            wrapped.push(0i64);
            wrapped.extend_from_slice(ids);
            wrapped.push(0i64);
            let n = wrapped.len() as i64;
            let ids_t = ort::value::Tensor::from_array((vec![1i64, n], wrapped)).map_err(oe)?;
            let style_t = ort::value::Tensor::from_array((vec![1i64, 256], style)).map_err(oe)?;
            let speed_t = ort::value::Tensor::from_array((vec![1i64], vec![speed])).map_err(oe)?;
            let outputs = self
                .session
                .run(ort::inputs![
                    self.ids_name.as_str() => ids_t,
                    self.style_name.as_str() => style_t,
                    self.speed_name.as_str() => speed_t,
                ])
                .map_err(oe)?;
            let (_shape, data) = outputs[self.out_name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(oe)?;
            Ok(data.to_vec())
        }
    }

    fn engine() -> &'static Mutex<Option<Engine>> {
        static E: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();
        E.get_or_init(|| Mutex::new(None))
    }

    /// Synthesize `text` in `voice` at `speed` (1.0 = normal) → 24 kHz mono i16.
    pub fn synth(text: &str, voice: &str, speed: f32) -> Result<Vec<i16>> {
        let phon = super::phonemize(text, super::voice_lang(voice));
        let ids = super::tokens(&phon);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = engine().lock().unwrap();
        if guard.is_none() {
            *guard = Some(Engine::load()?);
        }
        let wave = guard.as_mut().unwrap().run(&ids, voice, speed.clamp(0.5, 2.0))?;
        Ok(wave
            .into_iter()
            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect())
    }
}

/// Synthesize a sentence to 24 kHz mono i16 PCM. Errors (or the feature being
/// off) leave the caller to fall back to the timer highlight.
pub fn synth(text: &str, voice: &str, speed: f32) -> anyhow::Result<Vec<i16>> {
    #[cfg(feature = "kokoro")]
    {
        engine::synth(text, voice, speed)
    }
    #[cfg(not(feature = "kokoro"))]
    {
        let _ = (text, voice, speed);
        anyhow::bail!("kokoro feature not built")
    }
}

/// Write mono 24 kHz i16 samples to a 16-bit PCM WAV file.
pub fn write_wav(samples: &[i16], path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let data_len = (samples.len() * 2) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?; // byte rate
    f.write_all(&2u16.to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?; // bits per sample
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocab_has_pad_zero_and_space() {
        let v = vocab();
        assert_eq!(v.get(&'$'), Some(&0));
        assert!(v.contains_key(&' '));
        assert!(v.contains_key(&'ɪ')); // an IPA symbol
    }

    #[test]
    fn tokens_drop_unknown() {
        // '§' isn't in the vocab; 'a' and ' ' are.
        let t = tokens("a§ b");
        assert!(!t.is_empty());
    }

    #[test]
    fn wav_header_is_44_bytes() {
        let dir = std::env::temp_dir();
        let p = dir.join("tulipix-kokoro-test.wav");
        write_wav(&[0, 1, -1, 100], &p).unwrap();
        let meta = std::fs::metadata(&p).unwrap();
        assert_eq!(meta.len(), 44 + 4 * 2);
        let _ = std::fs::remove_file(&p);
    }
}
