// SPDX-License-Identifier: AGPL-3.0-only
//! Places for find_places, from OpenStreetMap: Nominatim for names and
//! addresses ("Walmart", "Eiffel Tower", "123 Main St, Provo"), Overpass for
//! kinds of places near a point ("coffee near me"). Nominatim allows one
//! request a second, so requests to it are spaced out.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A place found.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub name: String,
    pub kind: String,
    pub address: String,
    pub lat: f64,
    pub lon: f64,
    pub distance_km: Option<f64>,
    pub hours: String,
    pub phone: String,
    pub website: String,
}

/// Kinds of place people ask for, as OpenStreetMap tag filters for Overpass.
const KINDS: &[(&str, &str, &str)] = &[
    // (words, filter, what it is)
    ("coffee|cafe|café|cafes|espresso|coffee shop", r#"["amenity"="cafe"]"#, "café"),
    ("pizza", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"pizza",i]"#, "pizza place"),
    ("burger|burgers", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"burger",i]"#, "burger place"),
    ("sushi", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"sushi|japanese",i]"#, "sushi restaurant"),
    ("mexican|tacos?", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"mexican|taco",i]"#, "Mexican restaurant"),
    ("chinese", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"chinese",i]"#, "Chinese restaurant"),
    ("thai", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"thai",i]"#, "Thai restaurant"),
    ("indian", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"indian",i]"#, "Indian restaurant"),
    ("italian", r#"["amenity"~"^(restaurant|fast_food)$"]["cuisine"~"italian|pizza",i]"#, "Italian restaurant"),
    ("fast food", r#"["amenity"="fast_food"]"#, "fast food"),
    ("restaurants?|food|places? to eat|dinner|lunch|breakfast", r#"["amenity"~"^(restaurant|fast_food)$"]"#, "restaurant"),
    ("bars?|pubs?|brewery", r#"["amenity"~"^(bar|pub|biergarten)$"]"#, "bar"),
    ("ice cream|frozen yogurt", r#"["amenity"="ice_cream"]"#, "ice cream shop"),
    ("bakery|bakeries|donuts?", r#"["shop"~"^(bakery|pastry|confectionery)$"]"#, "bakery"),
    ("gas|gas stations?|petrol|fuel", r#"["amenity"="fuel"]"#, "gas station"),
    (r"ev charg\w*|charging stations?|chargers?", r#"["amenity"="charging_station"]"#, "charging station"),
    ("car wash", r#"["amenity"="car_wash"]"#, "car wash"),
    ("parking", r#"["amenity"="parking"]"#, "parking"),
    ("pharmacy|pharmacies|drugstore", r#"["amenity"="pharmacy"]"#, "pharmacy"),
    ("hospitals?|emergency room", r#"["amenity"="hospital"]"#, "hospital"),
    ("urgent care|clinics?|doctors?", r#"["amenity"~"^(clinic|doctors)$"]"#, "clinic"),
    ("dentists?", r#"["amenity"="dentist"]"#, "dentist"),
    (r"vets?|veterinar\w*", r#"["amenity"="veterinary"]"#, "vet"),
    (r"grocer\w*|supermarkets?", r#"["shop"~"^(supermarket|grocery)$"]"#, "grocery store"),
    ("convenience stores?", r#"["shop"="convenience"]"#, "convenience store"),
    ("hardware stores?", r#"["shop"~"^(hardware|doityourself)$"]"#, "hardware store"),
    ("laundromats?|laundry", r#"["shop"~"^(laundry|dry_cleaning)$"]"#, "laundromat"),
    ("banks?", r#"["amenity"="bank"]"#, "bank"),
    ("atms?|cash machine", r#"["amenity"="atm"]"#, "ATM"),
    ("parks?|playgrounds?", r#"["leisure"~"^(park|playground)$"]"#, "park"),
    ("gyms?|fitness", r#"["leisure"="fitness_centre"]"#, "gym"),
    ("hotels?|motels?|lodging|places? to stay", r#"["tourism"~"^(hotel|motel|guest_house|hostel)$"]"#, "hotel"),
    (r"librar\w*", r#"["amenity"="library"]"#, "library"),
    ("schools?", r#"["amenity"="school"]"#, "school"),
    ("post offices?", r#"["amenity"="post_office"]"#, "post office"),
    ("movie theaters?|cinemas?|movies", r#"["amenity"="cinema"]"#, "cinema"),
    ("museums?", r#"["tourism"="museum"]"#, "museum"),
    ("churches|church|places? of worship|temples?|mosques?", r#"["amenity"="place_of_worship"]"#, "place of worship"),
    ("police", r#"["amenity"="police"]"#, "police station"),
    ("bus stops?", r#"["highway"="bus_stop"]"#, "bus stop"),
    ("restrooms?|toilets?|bathrooms?", r#"["amenity"="toilets"]"#, "restroom"),
];

/// The kind of place a search names ("coffee near me" → cafés), if it's one.
pub fn kind_of(query: &str) -> Option<(&'static str, &'static str)> {
    let q = query.to_lowercase();
    KINDS.iter().find(|(words, _, _)| regex::Regex::new(&format!(r"\b({words})\b")).is_ok_and(|re| re.is_match(&q))).map(|(_, filter, what)| (*filter, *what))
}

/// Kilometers between two points.
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().asin()
}

/// Nominatim's limit: one request a second.
async fn nominatim_turn() {
    static LAST: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Default::default);
    let wait = {
        let mut last = LAST.lock().unwrap();
        let now = Instant::now();
        let next = last.map_or(now, |t| (t + Duration::from_millis(1100)).max(now));
        *last = Some(next);
        next - now
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

fn address_of(tags: &Value) -> String {
    let t = |k: &str| tags[k].as_str().unwrap_or("").trim().to_string();
    let street = [t("addr:housenumber"), t("addr:street")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    [street, t("addr:city")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
}

/// A city or address's coordinates and full name.
pub async fn geocode(client: &reqwest::Client, place: &str) -> Result<Option<(f64, f64, String)>, String> {
    nominatim_turn().await;
    let v: Value = client
        .get("https://nominatim.openstreetmap.org/search")
        .query(&[("q", place), ("format", "jsonv2"), ("limit", "1")])
        .send()
        .await
        .map_err(|e| format!("Couldn't reach OpenStreetMap ({}).", e.without_url()))?
        .json()
        .await
        .map_err(|_| "OpenStreetMap's answer was unreadable.".to_string())?;
    let first = &v[0];
    let num = |k: &str| first[k].as_str().and_then(|s| s.parse::<f64>().ok());
    Ok(match (num("lat"), num("lon")) {
        (Some(lat), Some(lon)) => Some((lat, lon, first["display_name"].as_str().unwrap_or(place).to_string())),
        _ => None,
    })
}

/// Places of a kind (an Overpass filter) within `radius_km` of a point, nearest first.
pub async fn nearby(client: &reqwest::Client, filter: &str, what: &str, center: (f64, f64), radius_km: f64) -> Result<Vec<Spot>, String> {
    let query = format!(
        "[out:json][timeout:20];nwr{filter}(around:{},{},{});out center tags 80;",
        (radius_km * 1000.0).round(),
        center.0,
        center.1
    );
    // The main server is often busy: a second public one then.
    let mut last = String::new();
    for server in ["https://overpass-api.de/api/interpreter", "https://overpass.kumi.systems/api/interpreter"] {
        let res = match client.post(server).form(&[("data", query.as_str())]).timeout(Duration::from_secs(15)).send().await {
            Ok(r) => r,
            Err(e) => {
                last = format!("Couldn't reach OpenStreetMap's place search ({}).", e.without_url());
                continue;
            }
        };
        if !res.status().is_success() {
            last = format!("OpenStreetMap's place search is busy ({}); try again in a minute.", res.status());
            continue;
        }
        match res.json::<Value>().await {
            Ok(v) if v["elements"].is_array() => return Ok(spots_of(&v, what, center)),
            _ => last = "OpenStreetMap's place search is busy; try again in a minute.".into(),
        }
    }
    Err(last)
}

/// Overpass elements as places, nearest first (unnamed ones only for kinds that rarely have names).
pub fn spots_of(v: &Value, what: &str, center: (f64, f64)) -> Vec<Spot> {
    let unnamed_ok = matches!(what, "ATM" | "parking" | "restroom" | "bus stop" | "charging station" | "park");
    let mut out: Vec<Spot> = v["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let tags = &e["tags"];
            let lat = e["lat"].as_f64().or_else(|| e["center"]["lat"].as_f64())?;
            let lon = e["lon"].as_f64().or_else(|| e["center"]["lon"].as_f64())?;
            let name = tags["name"].as_str().or_else(|| tags["brand"].as_str()).map(str::to_string);
            if name.is_none() && !unnamed_ok {
                return None;
            }
            let t = |k: &str| tags[k].as_str().unwrap_or("").to_string();
            Some(Spot {
                name: name.unwrap_or_else(|| what.to_string()),
                kind: what.to_string(),
                address: address_of(tags),
                lat,
                lon,
                distance_km: Some(distance_km(center, (lat, lon))),
                hours: t("opening_hours"),
                phone: if t("phone").is_empty() { t("contact:phone") } else { t("phone") },
                website: if t("website").is_empty() { t("contact:website") } else { t("website") },
            })
        })
        .collect();
    out.sort_by(|a, b| a.distance_km.partial_cmp(&b.distance_km).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| a.name == b.name && a.distance_km.zip(b.distance_km).is_some_and(|(x, y)| (x - y).abs() < 0.05));
    out.truncate(10);
    out
}

/// A business, landmark or address by name: near a point first when one is
/// known (about 45 km around), anywhere if nothing nearby matches.
pub async fn by_name(client: &reqwest::Client, query: &str, center: Option<(f64, f64)>) -> Result<Vec<Spot>, String> {
    if center.is_some() {
        let near = search_named(client, query, center, Some(0.4)).await?;
        if !near.is_empty() {
            return Ok(near);
        }
    }
    search_named(client, query, center, None).await
}

/// What Nominatim calls a kind of place (its search knows "cafe" and "fuel").
fn phrase_for(what: &str) -> &str {
    match what {
        "café" => "cafe",
        "pizza place" => "pizza",
        "burger place" => "burger",
        "sushi restaurant" => "sushi",
        "gas station" => "fuel",
        "grocery store" => "supermarket",
        "laundromat" => "laundry",
        "place of worship" => "place of worship",
        "restroom" => "toilets",
        "vet" => "veterinary",
        "gym" => "fitness centre",
        "ice cream shop" => "ice cream",
        "police station" => "police",
        other => other,
    }
}

/// Places of a kind near a point from Nominatim, for when Overpass (often
/// overloaded) can't answer: fewer and less tidy, but nearest first.
pub async fn nearby_by_name(client: &reqwest::Client, what: &str, center: (f64, f64), radius_km: f64) -> Result<Vec<Spot>, String> {
    let deg = (radius_km * 1.5 / 111.0).clamp(0.03, 0.4);
    let mut spots = search_named(client, phrase_for(what), Some(center), Some(deg)).await?;
    for s in spots.iter_mut() {
        s.kind = what.to_string();
    }
    Ok(spots)
}

async fn search_named(client: &reqwest::Client, query: &str, center: Option<(f64, f64)>, box_deg: Option<f64>) -> Result<Vec<Spot>, String> {
    nominatim_turn().await;
    let mut params: Vec<(&str, String)> = vec![("q", query.into()), ("format", "jsonv2".into()), ("limit", "10".into()), ("addressdetails", "1".into()), ("extratags", "1".into())];
    if let (Some((lat, lon)), Some(d)) = (center, box_deg) {
        params.push(("viewbox", format!("{},{},{},{}", lon - d, lat + d, lon + d, lat - d)));
        params.push(("bounded", "1".into()));
    }
    let v: Value = client
        .get("https://nominatim.openstreetmap.org/search")
        .query(&params)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach OpenStreetMap ({}).", e.without_url()))?
        .json()
        .await
        .map_err(|_| "OpenStreetMap's answer was unreadable.".to_string())?;
    let mut out: Vec<Spot> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let lat = p["lat"].as_str()?.parse::<f64>().ok()?;
            let lon = p["lon"].as_str()?.parse::<f64>().ok()?;
            let a = &p["address"];
            let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
            let street = [s("house_number"), s("road")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
            let town = [s("city"), s("town"), s("village")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
            let address = [street, town, s("state")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ");
            let name = p["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| p["display_name"].as_str().unwrap_or("").to_string());
            let x = |k: &str| p["extratags"][k].as_str().unwrap_or("").to_string();
            Some(Spot {
                name,
                kind: p["type"].as_str().unwrap_or("").replace('_', " "),
                address: if address.is_empty() { p["display_name"].as_str().unwrap_or("").to_string() } else { address },
                lat,
                lon,
                distance_km: center.map(|c| distance_km(c, (lat, lon))),
                hours: x("opening_hours"),
                phone: x("phone"),
                website: x("website"),
            })
        })
        .collect();
    if center.is_some() {
        out.sort_by(|a, b| a.distance_km.partial_cmp(&b.distance_km).unwrap_or(std::cmp::Ordering::Equal));
    }
    out.truncate(6);
    Ok(out)
}

/// A map link for a place, and directions from `from` when it's known.
pub fn links(s: &Spot, from: Option<(f64, f64)>) -> (String, Option<String>) {
    let map = format!("https://www.openstreetmap.org/?mlat={:.6}&mlon={:.6}#map=18/{:.6}/{:.6}", s.lat, s.lon, s.lat, s.lon);
    let route = from.map(|(la, lo)| format!("https://www.openstreetmap.org/directions?route={la:.6}%2C{lo:.6}%3B{:.6}%2C{:.6}", s.lat, s.lon));
    (map, route)
}

/// Within the US (continental, Alaska or Hawaii), where distances are in miles.
pub fn in_us(lat: f64, lon: f64) -> bool {
    (24.0..50.0).contains(&lat) && (-125.0..-66.0).contains(&lon) || (51.0..72.0).contains(&lat) && (-170.0..-129.0).contains(&lon) || (18.0..23.0).contains(&lat) && (-161.0..-154.0).contains(&lon)
}

/// Distances the way people say them where they are (miles in the US).
pub fn distance_text(km: f64, miles: bool) -> String {
    if miles {
        let mi = km / 1.609_344;
        if mi < 0.1 { format!("{} ft", (mi * 5280.0 / 10.0).round() * 10.0) } else { format!("{mi:.1} mi") }
    } else if km < 1.0 {
        format!("{} m", (km * 100.0).round() * 10.0)
    } else {
        format!("{km:.1} km")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_of_places_people_ask_for() {
        assert_eq!(kind_of("coffee near me").map(|k| k.1), Some("café"));
        assert_eq!(kind_of("Where's the closest gas station?").map(|k| k.1), Some("gas station"));
        assert_eq!(kind_of("good pizza in Provo").map(|k| k.1), Some("pizza place"));
        assert_eq!(kind_of("an ATM").map(|k| k.1), Some("ATM"));
        assert_eq!(kind_of("Walmart"), None);
        assert_eq!(kind_of("Eiffel Tower"), None);
    }

    /// Live: OpenStreetMap's servers (run with --ignored).
    #[tokio::test]
    #[ignore]
    async fn finds_real_places_near_orem() {
        let client = reqwest::Client::builder().user_agent(concat!("SulcusAI/", env!("CARGO_PKG_VERSION"), " (test)")).build().unwrap();
        let orem = (40.2969, -111.6946);
        let (filter, what) = kind_of("coffee").unwrap();
        // Overpass is often overloaded; the tool falls back to Nominatim then.
        let cafes = match nearby(&client, filter, what, orem, 3.0).await {
            Ok(c) if !c.is_empty() => c,
            _ => nearby_by_name(&client, what, orem, 3.0).await.unwrap(),
        };
        assert!(!cafes.is_empty() && cafes.windows(2).all(|w| w[0].distance_km <= w[1].distance_km), "{cafes:?}");
        let walmart = by_name(&client, "Walmart", Some(orem)).await.unwrap();
        assert!(walmart.first().is_some_and(|s| s.distance_km.unwrap_or(99.0) < 45.0), "the nearest, not one in another state: {walmart:?}");
        let fallback = nearby_by_name(&client, "gas station", orem, 3.0).await.unwrap();
        assert!(!fallback.is_empty() && fallback.iter().all(|s| s.distance_km.unwrap_or(99.0) < 10.0), "{fallback:?}");
        let (lat, lon, name) = geocode(&client, "Provo, Utah").await.unwrap().unwrap();
        assert!(name.contains("Provo") && distance_km(orem, (lat, lon)) < 15.0);
    }

    #[test]
    fn distances_and_nearest_first() {
        // Orem to Provo, Utah: about 7 km.
        let d = distance_km((40.2969, -111.6946), (40.2338, -111.6585));
        assert!((6.5..8.5).contains(&d), "{d}");
        assert_eq!(distance_text(0.5, true), "0.3 mi");
        assert_eq!(distance_text(0.42, false), "420 m");
        let v = serde_json::json!({ "elements": [
            { "type": "node", "lat": 40.31, "lon": -111.70, "tags": { "name": "Far Café", "opening_hours": "Mo-Fr 07:00-17:00" } },
            { "type": "way", "center": { "lat": 40.297, "lon": -111.695 }, "tags": { "name": "Near Café", "addr:housenumber": "12", "addr:street": "Main St", "addr:city": "Orem" } },
            { "type": "node", "lat": 40.298, "lon": -111.694, "tags": { "amenity": "cafe" } },
        ] });
        let s = spots_of(&v, "café", (40.2969, -111.6946));
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["Near Café", "Far Café"], "nearest first; an unnamed café is left out");
        assert_eq!(s[0].address, "12 Main St, Orem");
        assert_eq!(s[1].hours, "Mo-Fr 07:00-17:00");
        let (map, route) = links(&s[0], Some((40.2969, -111.6946)));
        assert!(map.contains("mlat=40.297000") && route.unwrap().contains("directions?route=40.296900%2C-111.694600%3B40.297000"));
    }
}
