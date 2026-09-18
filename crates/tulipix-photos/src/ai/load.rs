//! Turning an installed model blob into something that can run.
//!
//! This lived in `tulipix-sec-photos`, which is the Slint front end's section
//! crate — so the Flutter bridge, which does not depend on it, had no way to
//! build a tagger or a face pair and therefore no way to index anything. There
//! is nothing about *which file to load and how* that belongs to a front end;
//! it belongs beside `models.rs`, which decided where the file goes.
//!
//! Every constructor returns `Option` and `None` means "no model", never "the
//! model found nothing". The indexer leans on that distinction: a stage with no
//! model must leave its queue alone, because marking photos considered would
//! drain the queue to a wrong answer and leave the real model, when it arrives,
//! with no work to find.

use std::path::PathBuf;

/// Path to an installed model blob, or `None` when it has not been downloaded.
///
/// `<data>/models/<name>-<version>/<name>.onnx` — the layout
/// [`super::models::local_path`] writes to.
pub fn installed_model(name: &str, version: &str) -> Option<PathBuf> {
    let p = super::models::models_root()?
        .join(format!("{name}-{version}"))
        .join(format!("{name}.onnx"));
    p.exists().then_some(p)
}

/// The COCO-80 object tagger, when both the feature and the blob are present.
#[cfg(feature = "onnx")]
pub fn make_tagger() -> Option<Box<dyn super::tags::Tagger>> {
    let p = installed_model("yolox-s", "1.0.0")?;
    match super::onnx::OrtTagger::load(&p) {
        Ok(t) => Some(Box::new(t)),
        Err(e) => {
            tracing::error!(error = %e, "OrtTagger load failed");
            None
        }
    }
}
#[cfg(not(feature = "onnx"))]
pub fn make_tagger() -> Option<Box<dyn super::tags::Tagger>> {
    None
}

/// Face detection + recognition. Both or neither — detection alone fills
/// `faces` with rows that can never be clustered, so the People tab stays
/// empty while the queue reports itself done.
#[cfg(feature = "onnx")]
pub fn make_face_models() -> Option<(
    Box<dyn super::faces::FaceDetector>,
    Box<dyn super::faces::FaceEmbedder>,
)> {
    let det_path = installed_model("face-det-500m", "1.0.0")?;
    let rec_path = installed_model("face-rec-500m", "1.0.0")?;
    let det = super::onnx::OrtFaceDetector::load(&det_path)
        .map_err(|e| tracing::error!(error = %e, "OrtFaceDetector load failed"))
        .ok()?;
    let rec = super::onnx::OrtFaceEmbedder::load(&rec_path)
        .map_err(|e| tracing::error!(error = %e, "OrtFaceEmbedder load failed"))
        .ok()?;
    Some((Box::new(det), Box::new(rec)))
}
#[cfg(not(feature = "onnx"))]
pub fn make_face_models() -> Option<(
    Box<dyn super::faces::FaceDetector>,
    Box<dyn super::faces::FaceEmbedder>,
)> {
    None
}

/// Whether a stage has everything it needs to do real work right now.
///
/// The UI asks this to decide between "Run" and "Get model": a Run button that
/// can only raise an error is worse than no button.
pub fn stage_ready(stage: super::background::Stage) -> bool {
    use super::background::Stage;
    match stage {
        // Neither of these needs a model — they are EXIF reads and an FTS
        // insert, and they are why a fresh library is searchable at all.
        Stage::Exif | Stage::Fts => true,
        // Every other stage runs a model, and without the `onnx` feature the
        // constructors above are the stubs that answer None. Ungated, the UI
        // offered a Run that marked nothing and finished instantly, and the
        // queue it was meant to drain never moved.
        Stage::Faces => {
            cfg!(feature = "onnx")
                && installed_model("face-det-500m", "1.0.0").is_some()
                && installed_model("face-rec-500m", "1.0.0").is_some()
        }
        Stage::Tags => cfg!(feature = "onnx") && installed_model("yolox-s", "1.0.0").is_some(),
        // CLIP's weights and its tokenizer are one capability: the tokenizer is
        // a .json, so it is checked by hand rather than through
        // `installed_model`, which assumes .onnx.
        Stage::Clip => {
            cfg!(feature = "onnx")
                && installed_model("clip-vit-b32-q8", "1.0.0").is_some()
                && super::models::models_root()
                    .map(|d| {
                        d.join("clip-vit-b32-tokenizer-1.0.0")
                            .join("clip-vit-b32-tokenizer.json")
                            .exists()
                    })
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::background::Stage;

    #[test]
    fn stages_without_a_model_are_always_ready() {
        assert!(stage_ready(Stage::Exif));
        assert!(stage_ready(Stage::Fts));
    }

    #[test]
    fn model_stages_follow_what_is_on_disk() {
        // Whatever this machine has, the answer must equal the file check and
        // the feature together -- the UI gates its Run button on exactly this,
        // and a build with no inference in it can run neither.
        assert_eq!(
            stage_ready(Stage::Tags),
            cfg!(feature = "onnx") && installed_model("yolox-s", "1.0.0").is_some()
        );
    }
}
