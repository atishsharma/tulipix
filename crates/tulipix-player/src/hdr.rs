//! HDR routing — Dolby Vision / HDR10+ / HDR10 / HLG.
//!
//! Decision tree: source HDR + display capability → either pass-through (mpv
//! `target-trc=pq`/`auto-gamut`) or tone-map (`tone-mapping=bt.2390` to the
//! display's primaries). Output is the option set the controller pushes to
//! libmpv via `set_option`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceHdr {
    Sdr,
    Hdr10,
    Hdr10Plus,
    DolbyVision,
    Hlg,
}

impl SourceHdr {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "hdr10" => Self::Hdr10,
            "hdr10+" | "hdr10plus" => Self::Hdr10Plus,
            "dolby_vision" | "dovi" => Self::DolbyVision,
            "hlg" => Self::Hlg,
            _ => Self::Sdr,
        }
    }
    pub fn is_hdr(self) -> bool { !matches!(self, Self::Sdr) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayCaps {
    pub supports_hdr10: bool,
    pub supports_hdr10_plus: bool,
    pub supports_dolby_vision: bool,
    pub supports_hlg: bool,
    /// Peak luminance the panel can hit; used when tone-mapping.
    pub peak_nits: f32,
}

impl Default for DisplayCaps {
    fn default() -> Self {
        Self {
            supports_hdr10: false,
            supports_hdr10_plus: false,
            supports_dolby_vision: false,
            supports_hlg: false,
            peak_nits: 300.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrRoute {
    Passthrough,
    ToneMap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdrPlan {
    pub route: HdrRoute,
    /// mpv options the controller needs to set before play.
    pub mpv_opts: Vec<(String, String)>,
    pub reason: String,
}

pub fn plan(src: SourceHdr, display: &DisplayCaps) -> HdrPlan {
    if !src.is_hdr() {
        return HdrPlan {
            route: HdrRoute::Passthrough,
            mpv_opts: vec![("tone-mapping".into(), "auto".into())],
            reason: "SDR source — no HDR routing required".into(),
        };
    }

    let can_passthrough = match src {
        SourceHdr::DolbyVision => display.supports_dolby_vision,
        SourceHdr::Hdr10Plus   => display.supports_hdr10_plus || display.supports_hdr10,
        SourceHdr::Hdr10       => display.supports_hdr10,
        SourceHdr::Hlg         => display.supports_hlg,
        SourceHdr::Sdr         => false,
    };

    if can_passthrough {
        HdrPlan {
            route: HdrRoute::Passthrough,
            mpv_opts: vec![
                ("target-trc".into(), match src { SourceHdr::Hlg => "hlg".into(), _ => "pq".into() }),
                ("target-prim".into(), "bt.2020".into()),
                ("tone-mapping".into(), "auto".into()),
            ],
            reason: format!("display supports {:?} — passing through", src),
        }
    } else {
        HdrPlan {
            route: HdrRoute::ToneMap,
            mpv_opts: vec![
                ("target-trc".into(), "auto".into()),
                ("target-prim".into(), "bt.709".into()),
                ("tone-mapping".into(), "bt.2390".into()),
                ("target-peak".into(), format!("{:.0}", display.peak_nits.max(80.0))),
            ],
            reason: format!("display lacks {:?} — tone-mapping to {} nits", src, display.peak_nits as i32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_parse_known() {
        assert_eq!(SourceHdr::parse("hdr10"), SourceHdr::Hdr10);
        assert_eq!(SourceHdr::parse("dolby_vision"), SourceHdr::DolbyVision);
        assert_eq!(SourceHdr::parse("nothing"), SourceHdr::Sdr);
    }

    #[test]
    fn sdr_short_circuits_to_passthrough() {
        let d = DisplayCaps::default();
        let p = plan(SourceHdr::Sdr, &d);
        assert_eq!(p.route, HdrRoute::Passthrough);
    }

    #[test]
    fn hdr10_on_hdr10_display_passes_through() {
        let d = DisplayCaps { supports_hdr10: true, ..DisplayCaps::default() };
        let p = plan(SourceHdr::Hdr10, &d);
        assert_eq!(p.route, HdrRoute::Passthrough);
        assert!(p.mpv_opts.iter().any(|(k, v)| k == "target-trc" && v == "pq"));
    }

    #[test]
    fn dovi_on_hdr10_display_tonemaps() {
        let d = DisplayCaps { supports_hdr10: true, ..DisplayCaps::default() };
        let p = plan(SourceHdr::DolbyVision, &d);
        assert_eq!(p.route, HdrRoute::ToneMap);
        assert!(p.mpv_opts.iter().any(|(k, _)| k == "target-peak"));
    }

    #[test]
    fn hdr10_plus_falls_back_to_hdr10_display() {
        let d = DisplayCaps { supports_hdr10: true, ..DisplayCaps::default() };
        assert_eq!(plan(SourceHdr::Hdr10Plus, &d).route, HdrRoute::Passthrough);
    }
}
