//! `np.p5.music.output` — audio output device picker (sink + exclusive /
//! bit-perfect mode).
//!
//! Pure helpers only: parse `mpv --audio-device=help` into a device list and
//! build the mpv `--key=value` options to route playback to a chosen sink.
//! The actual enumeration call + IPC live in main.rs / tulipix-common.

#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

/// The always-available "let mpv decide" device.
pub fn default_device() -> AudioDevice {
    AudioDevice { id: "auto".into(), name: "Autoselect device".into() }
}

/// Parse the output of `mpv --audio-device=help`.
///
/// Lines look like:
/// ```text
/// List of detected audio devices:
///   'auto' (Autoselect device)
///   'pulse/alsa_output.pci-0000_00_1f.3.analog-stereo' (Built-in Audio Analog Stereo)
/// ```
pub fn parse_device_list(mpv_help_output: &str) -> Vec<AudioDevice> {
    let mut out = Vec::new();
    for line in mpv_help_output.lines() {
        let line = line.trim();
        // Each device line opens with a single-quoted id.
        let Some(rest) = line.strip_prefix('\'') else { continue };
        let Some(close) = rest.find('\'') else { continue };
        let id = rest[..close].to_string();
        if id.is_empty() { continue; }
        // Friendly name lives between the following parentheses, if present.
        let name = rest[close + 1..]
            .trim()
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| id.clone());
        out.push(AudioDevice { id, name });
    }
    out
}

/// mpv `--key=value` options to route audio to `device_id`. When `exclusive`
/// is set, request bit-perfect exclusive access to the sink.
pub fn device_options(device_id: &str, exclusive: bool) -> Vec<String> {
    let mut v = vec![format!("--audio-device={device_id}")];
    if exclusive {
        v.push("--audio-exclusive=yes".to_string());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "List of detected audio devices:\n  \
        'auto' (Autoselect device)\n  \
        'pulse/alsa_output.pci-0000_00_1f.3.analog-stereo' (Built-in Audio Analog Stereo)\n  \
        'alsa/default' (Default ALSA device)\n";

    #[test]
    fn parses_devices() {
        let d = parse_device_list(SAMPLE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0], AudioDevice { id: "auto".into(), name: "Autoselect device".into() });
        assert_eq!(d[1].id, "pulse/alsa_output.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(d[1].name, "Built-in Audio Analog Stereo");
    }

    #[test]
    fn skips_header_and_blanks() {
        assert!(parse_device_list("nothing here\n\n").is_empty());
        assert!(parse_device_list("").is_empty());
    }

    #[test]
    fn id_without_name_falls_back_to_id() {
        let d = parse_device_list("  'jack' ");
        assert_eq!(d, vec![AudioDevice { id: "jack".into(), name: "jack".into() }]);
    }

    #[test]
    fn options_with_and_without_exclusive() {
        assert_eq!(device_options("auto", false), vec!["--audio-device=auto".to_string()]);
        assert_eq!(
            device_options("pulse/x", true),
            vec!["--audio-device=pulse/x".to_string(), "--audio-exclusive=yes".to_string()]
        );
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(default_device().id, "auto");
    }
}
