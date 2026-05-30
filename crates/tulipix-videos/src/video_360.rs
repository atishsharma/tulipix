//! 360 / equirectangular video — detect the projection from sidecar XMP
//! (`GSpherical` block) or container metadata (`spherical` flag on the MKV /
//! MP4 track), then emit the mpv `--lavfi-complex` graph that reprojects each
//! frame to a flat viewport with mouse-drag / gyro-driven yaw/pitch/roll.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    Equirectangular,
    Cubemap,
    EquiAngularCubemap,
    /// 180° hemisphere (VR snippets shot on Insta360 / Vuze)
    Hemispherical180,
}

impl Projection {
    pub fn as_str(self) -> &'static str {
        match self {
            Projection::Equirectangular => "equirect",
            Projection::Cubemap => "c3x2",
            Projection::EquiAngularCubemap => "eac",
            Projection::Hemispherical180 => "hequirect",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SphericalInfo {
    pub projection: Option<Projection>,
    pub stereo: StereoLayout,
    pub initial_yaw_deg: f64,
    pub initial_pitch_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StereoLayout {
    #[default]
    Mono,
    TopBottom,
    LeftRight,
}

/// Parses the legacy Google `GSpherical` XML block stored as a side-data XMP
/// blob on Matroska + MP4. We only pull the fields mpv cares about — the rest
/// (timestamps, content matrix) are ignored.
pub fn parse_gspherical_xml(xml: &str) -> SphericalInfo {
    fn tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
        let open = format!("<GSpherical:{tag}>");
        let close = format!("</GSpherical:{tag}>");
        let start = xml.find(&open)? + open.len();
        let end = xml[start..].find(&close)?;
        Some(xml[start..start + end].trim())
    }
    let is_spherical = tag(xml, "Spherical").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);
    let projection_str = tag(xml, "ProjectionType").unwrap_or("equirectangular");
    let projection = if !is_spherical {
        None
    } else {
        Some(match projection_str.to_ascii_lowercase().as_str() {
            "cubemap" => Projection::Cubemap,
            "equi-angular-cubemap" | "eac" => Projection::EquiAngularCubemap,
            "half_equirectangular" => Projection::Hemispherical180,
            _ => Projection::Equirectangular,
        })
    };
    let stereo = match tag(xml, "StereoMode").unwrap_or("mono").to_ascii_lowercase().as_str() {
        "top-bottom" => StereoLayout::TopBottom,
        "left-right" => StereoLayout::LeftRight,
        _ => StereoLayout::Mono,
    };
    let initial_yaw_deg = tag(xml, "InitialViewHeadingDegrees")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let initial_pitch_deg = tag(xml, "InitialViewPitchDegrees")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    SphericalInfo {
        projection,
        stereo,
        initial_yaw_deg,
        initial_pitch_deg,
    }
}

/// Build the mpv `--lavfi-complex` argument that reprojects to a flat
/// 1920×1080 viewport. Returns `None` when no projection was detected — the
/// caller plays the file unmodified.
pub fn lavfi_complex(info: &SphericalInfo, yaw_deg: f64, pitch_deg: f64, roll_deg: f64) -> Option<String> {
    let p = info.projection?;
    let input_proj = match p {
        Projection::Equirectangular => "e",
        Projection::Cubemap => "c3x2",
        Projection::EquiAngularCubemap => "eac",
        Projection::Hemispherical180 => "hequirect",
    };
    let stereo_chain = match info.stereo {
        StereoLayout::Mono => String::new(),
        StereoLayout::TopBottom => "[vid1]crop=iw:ih/2:0:0[mono];[mono]".into(),
        StereoLayout::LeftRight => "[vid1]crop=iw/2:ih:0:0[mono];[mono]".into(),
    };
    Some(format!(
        "{stereo}v360={input_proj}:flat:yaw={yaw}:pitch={pitch}:roll={roll}:w=1920:h=1080[vo]",
        stereo = stereo_chain,
        input_proj = input_proj,
        yaw = yaw_deg,
        pitch = pitch_deg,
        roll = roll_deg,
    ))
}

/// Mini-map crosshair — return (x, y) of the current viewport centre on a
/// 0..1 normalised equirectangular minimap. Used by the UI overlay so the
/// user always knows which slice of the sphere they're staring at.
pub fn minimap_position(yaw_deg: f64, pitch_deg: f64) -> (f64, f64) {
    let yaw = ((yaw_deg % 360.0) + 360.0) % 360.0;
    let x = yaw / 360.0;
    let y = (pitch_deg.clamp(-90.0, 90.0) + 90.0) / 180.0;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<rdf:SphericalVideo
xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
xmlns:GSpherical="http://ns.google.com/videos/1.0/spherical/">
<GSpherical:Spherical>true</GSpherical:Spherical>
<GSpherical:Stitched>true</GSpherical:Stitched>
<GSpherical:StitchingSoftware>tulipix</GSpherical:StitchingSoftware>
<GSpherical:ProjectionType>equirectangular</GSpherical:ProjectionType>
<GSpherical:StereoMode>top-bottom</GSpherical:StereoMode>
<GSpherical:InitialViewHeadingDegrees>180</GSpherical:InitialViewHeadingDegrees>
<GSpherical:InitialViewPitchDegrees>-20</GSpherical:InitialViewPitchDegrees>
</rdf:SphericalVideo>"#;

    #[test]
    fn parse_equirect_top_bottom() {
        let info = parse_gspherical_xml(SAMPLE_XML);
        assert_eq!(info.projection, Some(Projection::Equirectangular));
        assert_eq!(info.stereo, StereoLayout::TopBottom);
        assert!((info.initial_yaw_deg - 180.0).abs() < 1e-9);
        assert!((info.initial_pitch_deg + 20.0).abs() < 1e-9);
    }

    #[test]
    fn non_spherical_returns_no_projection() {
        let xml = "<GSpherical:Spherical>false</GSpherical:Spherical>";
        assert!(parse_gspherical_xml(xml).projection.is_none());
    }

    #[test]
    fn unknown_projection_falls_back_to_equirect() {
        let xml = "<GSpherical:Spherical>true</GSpherical:Spherical>\
                   <GSpherical:ProjectionType>fisheye-marshmallow</GSpherical:ProjectionType>";
        assert_eq!(
            parse_gspherical_xml(xml).projection,
            Some(Projection::Equirectangular)
        );
    }

    #[test]
    fn lavfi_complex_includes_v360_and_stereo_crop() {
        let info = parse_gspherical_xml(SAMPLE_XML);
        let cx = lavfi_complex(&info, 30.0, -10.0, 0.0).unwrap();
        assert!(cx.contains("v360=e:flat"));
        assert!(cx.contains("yaw=30"));
        assert!(cx.contains("crop=iw:ih/2"));
    }

    #[test]
    fn lavfi_complex_none_when_not_spherical() {
        let info = SphericalInfo::default();
        assert!(lavfi_complex(&info, 0.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn minimap_position_wraps_yaw() {
        assert!((minimap_position(0.0, 0.0).0 - 0.0).abs() < 1e-9);
        assert!((minimap_position(360.0, 0.0).0 - 0.0).abs() < 1e-9);
        assert!((minimap_position(-90.0, 0.0).0 - 0.75).abs() < 1e-9);
        assert!((minimap_position(0.0, 90.0).1 - 1.0).abs() < 1e-9);
        assert!((minimap_position(0.0, -90.0).1 - 0.0).abs() < 1e-9);
    }
}
