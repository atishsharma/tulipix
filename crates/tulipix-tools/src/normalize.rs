//! `np.p4.tools.normalize` — audio normalise batch (EBU R128 / ReplayGain).
//!
//! Two modes: ffmpeg `loudnorm` (EBU R128 to a target LUFS) and ReplayGain tag
//! computation (gain = reference − measured). Owns the loudnorm filter string
//! (incl. two-pass measured-values injection) and the RG gain math.

pub const EBU_TARGET_LUFS: f64 = -23.0;     // EBU R128 broadcast
pub const REPLAYGAIN_REF_LUFS: f64 = -18.0; // ReplayGain 2.0 reference

/// First-pass `loudnorm` filter (measurement only, prints JSON).
pub fn loudnorm_measure_filter(target_lufs: f64) -> String {
    format!("loudnorm=I={target_lufs}:TP=-1.5:LRA=11:print_format=json")
}

/// Second-pass `loudnorm` filter with measured values from pass 1.
pub fn loudnorm_apply_filter(target_lufs: f64, measured_i: f64, measured_tp: f64, measured_lra: f64, measured_thresh: f64) -> String {
    format!(
        "loudnorm=I={target_lufs}:TP=-1.5:LRA=11:measured_I={measured_i}:measured_TP={measured_tp}:measured_LRA={measured_lra}:measured_thresh={measured_thresh}:linear=true"
    )
}

/// ReplayGain track gain (dB) for a measured integrated loudness.
pub fn replaygain_db(measured_lufs: f64) -> f64 {
    REPLAYGAIN_REF_LUFS - measured_lufs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudnorm_two_pass() {
        assert!(loudnorm_measure_filter(EBU_TARGET_LUFS).contains("print_format=json"));
        let a = loudnorm_apply_filter(-23.0, -19.0, -2.0, 8.0, -30.0);
        assert!(a.contains("measured_I=-19"));
        assert!(a.contains("linear=true"));
    }

    #[test]
    fn replaygain_math() {
        // quiet track (-24 LUFS) gets +6 dB toward -18 reference
        assert!((replaygain_db(-24.0) - 6.0).abs() < 1e-9);
        // loud track (-12) gets -6 dB
        assert!((replaygain_db(-12.0) + 6.0).abs() < 1e-9);
    }
}
