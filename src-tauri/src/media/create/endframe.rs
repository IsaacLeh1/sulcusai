// SPDX-License-Identifier: AGPL-3.0-only
//! Video ending on a chosen picture. sd.cpp takes `--end-img` with Wan 2.2
//! (FLF2V); other video models ignore it, so ask before promising it.

/// Whether a video model id can end on a chosen picture.
pub fn supports_end(model_id: &str) -> bool {
    let m = model_id.to_ascii_lowercase();
    (m.contains("wan2.2") || m.contains("wan-2.2") || m.contains("wan22")) && !m.contains("s2v")
        || m.contains("flf2v")
}

/// Both pictures must share a size or the model stretches the end one.
pub fn sizes_match(start: (u32, u32), end: (u32, u32)) -> Result<(), String> {
    if start == end {
        Ok(())
    } else {
        Err(format!("The end picture is {}x{} but the start is {}x{}. Crop it to the same size first.", end.0, end.1, start.0, start.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wan_supports() {
        assert!(supports_end("QuantStack/Wan2.2-TI2V-5B-GGUF"));
        assert!(supports_end("wan2.2-i2v-a14b"));
        assert!(!supports_end("Wan2.2-S2V-14B"));
        assert!(!supports_end("ltx-2.3"));
    }
    #[test]
    fn sizes() {
        assert!(sizes_match((832, 480), (832, 480)).is_ok());
        assert!(sizes_match((832, 480), (480, 832)).is_err());
    }
}
