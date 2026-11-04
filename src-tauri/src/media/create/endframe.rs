// SPDX-License-Identifier: AGPL-3.0-only
//! Video ending on a chosen picture. sd.cpp takes `--end-img`, but only
//! first-and-last-frame models use it (Wan 2.2 I2V A14B); Wan 2.2 TI2V
//! ignored it and LTX-2.3 came out blank, so the catalog's "end" says which.

/// Whether a video model (by its catalog abilities) can end on a picture.
pub fn supports_end(can: &[String]) -> bool {
    can.iter().any(|c| c == "end")
}

/// Both pictures must share a shape or the model stretches the end one.
pub fn sizes_match(start: (u32, u32), end: (u32, u32)) -> Result<(), String> {
    let r = |(w, h): (u32, u32)| w as f64 / h.max(1) as f64;
    if (r(start) - r(end)).abs() < 0.02 {
        Ok(())
    } else {
        Err(format!("The end picture is {}x{} but the start is {}x{}. Crop it to the same shape first.", end.0, end.1, start.0, start.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follows_catalog() {
        assert!(supports_end(&["image".into(), "end".into()]));
        assert!(!supports_end(&["text".into(), "image".into()]));
    }
    #[test]
    fn catalog_has_an_end_model() {
        let cat: serde_json::Value = serde_json::from_str(include_str!("../../../../catalog/catalog.json")).unwrap();
        let models = cat["media"]["models"].as_array().unwrap();
        assert!(models.iter().any(|m| m["can"].as_array().unwrap().iter().any(|c| c == "end")));
    }
    #[test]
    fn sizes() {
        assert!(sizes_match((832, 480), (1664, 960)).is_ok());
        assert!(sizes_match((832, 480), (480, 832)).is_err());
    }
}
