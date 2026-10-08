// SPDX-License-Identifier: AGPL-3.0-only
//! Where the user is, so "what's the weather today?" needs no follow-up.
//!
//! The place they saved in Settings › About you comes first. Without one,
//! Windows location services are asked (when the user allows apps to use
//! location in Windows' privacy settings). Coordinates stay on this PC; only
//! the weather request carries them, and only with web access on.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Place {
    pub label: String,
    pub lat: f64,
    pub lon: f64,
}

fn cache() -> &'static Mutex<Option<(Instant, Place)>> {
    static C: std::sync::OnceLock<Mutex<Option<(Instant, Place)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// This PC's position from Windows location services (cached 30 minutes).
pub async fn detect() -> Result<Place, String> {
    if let Some((at, p)) = cache().lock().unwrap().clone() {
        if at.elapsed() < Duration::from_secs(1800) {
            return Ok(p);
        }
    }
    let (lat, lon) = tokio::task::spawn_blocking(windows_position).await.map_err(|e| e.to_string())??;
    let p = Place { label: "your current location".into(), lat, lon };
    *cache().lock().unwrap() = Some((Instant::now(), p.clone()));
    Ok(p)
}

#[cfg(windows)]
fn windows_position() -> Result<(f64, f64), String> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};
    use windows::Foundation::TimeSpan;
    let off = "Windows location is off for apps. Turn on Settings › Privacy & security › Location, including “Let desktop apps access your location”, or type your city in SulcusAI's Settings › About you.";
    let access = Geolocator::RequestAccessAsync().and_then(|op| op.get()).map_err(|_| off.to_string())?;
    if access != GeolocationAccessStatus::Allowed {
        return Err(off.into());
    }
    let g = Geolocator::new().map_err(|e| e.to_string())?;
    // Up to 30 minutes old is fine; give up after 10 seconds.
    let age = TimeSpan { Duration: 30 * 60 * 10_000_000 };
    let timeout = TimeSpan { Duration: 10 * 10_000_000 };
    let pos = g
        .GetGeopositionAsyncWithAgeAndTimeout(age, timeout)
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows couldn't tell where this PC is ({}). Type your city in Settings › About you instead.", e.message()))?;
    let p = pos.Coordinate().and_then(|c| c.Point()).and_then(|pt| pt.Position()).map_err(|e| e.to_string())?;
    Ok((p.Latitude, p.Longitude))
}

#[cfg(not(windows))]
fn windows_position() -> Result<(f64, f64), String> {
    Err("Type your city in Settings › About you.".into())
}

/// A readable name for coordinates (OpenStreetMap's Nominatim; only when
/// the user clicks “Use this PC's location” with web access on).
pub async fn name_of(client: &reqwest::Client, lat: f64, lon: f64) -> Option<String> {
    let v: serde_json::Value = client
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(&[("lat", lat.to_string()), ("lon", lon.to_string()), ("format", "jsonv2".into()), ("zoom", "10".into())])
        .header("Accept-Language", "en")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let a = &v["address"];
    let city = ["city", "town", "village", "municipality", "county"].iter().find_map(|k| a[*k].as_str())?;
    let region = a["state"].as_str().or(a["country"].as_str());
    Some(match region {
        Some(r) => format!("{city}, {r}"),
        None => city.to_string(),
    })
}

/// "here", "my location" and the like: the user's own place.
pub fn means_here(place: &str) -> bool {
    let p = place.trim().trim_end_matches(['.', '?', '!']).to_lowercase();
    p.is_empty()
        || matches!(
            p.as_str(),
            "here" | "my location" | "current location" | "my current location" | "my area" | "near me" | "nearby" | "my city" | "local" | "where i am" | "home" | "user location" | "the user's location"
        )
}

/// Whether the user named this place themselves (its first part appears in
/// what they wrote), so a place a model made up isn't used.
pub fn named_in(place: &str, user_text: &str) -> bool {
    let city = place.split(',').next().unwrap_or("").trim().to_lowercase();
    !city.is_empty() && user_text.to_lowercase().contains(&city)
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Detected {
    label: String,
    lat: f64,
    lon: f64,
}

/// “Use this PC's location” in Settings: the position, named when web
/// access allows looking the name up.
#[tauri::command]
pub async fn detect_location(state: crate::AppStateRef<'_>) -> Result<Detected, String> {
    state.cipher()?;
    let p = detect().await?;
    let level = state.settings().connectivity;
    let label = match crate::net::external_client(level, crate::net::Purpose::Search, false) {
        Ok(client) => name_of(&client, p.lat, p.lon).await,
        Err(_) => None,
    }
    .unwrap_or_else(|| format!("{:.3}, {:.3}", p.lat, p.lon));
    crate::db::log_action(&state.db.lock().unwrap(), "privacy", "Looked up this PC's location for Settings");
    Ok(Detected { label, lat: p.lat, lon: p.lon })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn here_means_the_users_own_place() {
        assert!(means_here(""));
        assert!(means_here("My location"));
        assert!(means_here("here?"));
        assert!(!means_here("Orem, Utah"));
        assert!(named_in("Orem, Utah", "what is the weather in orem utah today"));
        assert!(!named_in("Toronto, Canada", "what is the weather today?"), "a place the model made up");
    }

    /// Asks Windows where this PC is: `cargo test --lib detects_this_pc -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn detects_this_pc() {
        println!("{:?}", detect().await);
    }
}
