//! `np.p4.music.stem` — AI stem separation (karaoke / DJ).
//!
//! Runs a local ONNX model (Demucs / Spleeter) to split a track into stems.
//! Inference is dispatched to the Tools queue; this module owns the stem
//! taxonomy, the per-model output-file naming, and the karaoke (drop-vocals)
//! mix selection so the player and Tools agree on file paths.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stem { Vocals, Drums, Bass, Other }

impl Stem {
    pub fn name(self) -> &'static str {
        match self { Stem::Vocals => "vocals", Stem::Drums => "drums", Stem::Bass => "bass", Stem::Other => "other" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Model { Demucs4Stem, Spleeter2Stem }

impl Model {
    pub fn id(self) -> &'static str {
        match self { Model::Demucs4Stem => "htdemucs", Model::Spleeter2Stem => "spleeter:2stems" }
    }
    /// Which stems this model emits.
    pub fn stems(self) -> Vec<Stem> {
        match self {
            Model::Demucs4Stem => vec![Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other],
            Model::Spleeter2Stem => vec![Stem::Vocals, Stem::Other], // vocals + accompaniment
        }
    }
}

/// Output path for one stem under `out_dir`: `<stem>/<track-stem>.wav`.
pub fn stem_path(out_dir: &Path, src: &Path, model: Model, stem: Stem) -> PathBuf {
    let base = src.file_stem().and_then(|s| s.to_str()).unwrap_or("track");
    out_dir.join(model.id()).join(stem.name()).join(format!("{base}.wav"))
}

/// Stems that make up a karaoke (instrumental) mix — everything but vocals.
pub fn karaoke_stems(model: Model) -> Vec<Stem> {
    model.stems().into_iter().filter(|s| *s != Stem::Vocals).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demucs_has_four_stems() {
        assert_eq!(Model::Demucs4Stem.stems().len(), 4);
        assert_eq!(Model::Spleeter2Stem.stems().len(), 2);
    }

    #[test]
    fn karaoke_drops_vocals() {
        let k = karaoke_stems(Model::Demucs4Stem);
        assert!(!k.contains(&Stem::Vocals));
        assert_eq!(k.len(), 3);
    }

    #[test]
    fn path_layout() {
        let p = stem_path(Path::new("/out"), Path::new("/m/Song.flac"), Model::Demucs4Stem, Stem::Bass);
        assert_eq!(p, PathBuf::from("/out/htdemucs/bass/Song.wav"));
    }
}
