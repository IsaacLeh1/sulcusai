// SPDX-License-Identifier: AGPL-3.0-only
//! Playlists: named lists of songs from the gallery, in the person's order,
//! kept encrypted in the settings. A song deleted from the gallery leaves
//! every playlist. And My taste: what their liked and made songs share.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::store::MediaItem;
use crate::crypto::Cipher;
use crate::db;

const KEY: &str = "music_playlists";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    /// Gallery ids, in playing order.
    pub songs: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Every playlist, the last changed first.
pub fn list(conn: &Connection, c: &Cipher) -> Vec<Playlist> {
    let mut all: Vec<Playlist> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[Playlist]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

/// Saves a playlist (new without an id). Two can't share a name; a song
/// is in a playlist once.
pub fn save(conn: &Connection, c: &Cipher, mut p: Playlist) -> Result<Playlist, String> {
    p.name = p.name.trim().chars().take(60).collect();
    if p.name.is_empty() {
        return Err("Give the playlist a name.".into());
    }
    let mut all = list(conn, c);
    if all.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name) && o.id != p.id) {
        return Err(format!("There's already a playlist called {}.", p.name));
    }
    let mut seen = std::collections::HashSet::new();
    p.songs.retain(|s| seen.insert(s.clone()));
    let now = db::now_ms();
    p.updated_at = now;
    match all.iter_mut().find(|o| o.id == p.id && !p.id.is_empty()) {
        Some(o) => {
            p.created_at = o.created_at;
            *o = p.clone();
        }
        None => {
            p.id = uuid::Uuid::new_v4().to_string();
            p.created_at = now;
            all.push(p.clone());
        }
    }
    store(conn, c, &all)?;
    Ok(p)
}

pub fn remove(conn: &Connection, c: &Cipher, id: &str) -> Result<(), String> {
    let mut all = list(conn, c);
    all.retain(|p| p.id != id);
    store(conn, c, &all)
}

/// A song deleted from the gallery, taken out of every playlist.
pub fn forget_song(conn: &Connection, c: &Cipher, song: &str) {
    let mut all = list(conn, c);
    if all.iter().any(|p| p.songs.iter().any(|s| s == song)) {
        for p in &mut all {
            p.songs.retain(|s| s != song);
        }
        let _ = store(conn, c, &all);
    }
}

/// What the person likes in songs, from their own (Suno's My taste): each
/// choice in a song's settings counted by how they felt about it, a liked
/// song 3, one made 1, one marked "not for me" -3.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Taste {
    pub genres: Vec<String>,
    pub moods: Vec<String>,
    pub instruments: Vec<String>,
    pub voice: Option<String>,
    pub era: Option<String>,
    pub production: Option<String>,
    /// Styles and instruments in the songs they marked, never in ones they liked.
    pub avoid: Vec<String>,
    /// The songs it's from.
    pub songs: usize,
}

pub fn taste(items: &[MediaItem]) -> Taste {
    use std::collections::HashMap;
    let mut scores: [HashMap<String, (i32, String)>; 6] = Default::default();
    let mut songs = 0;
    for it in items.iter().filter(|i| i.kind == "audio" && i.op == "music") {
        let Some(m) = it.music.as_ref() else { continue };
        songs += 1;
        let w = if it.disliked { -3 } else if it.favorite { 3 } else { 1 };
        let listed = |s: &Option<String>| s.as_deref().unwrap_or_default().split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect::<Vec<_>>();
        let fields: [Vec<String>; 6] = [listed(&m.genre), listed(&m.mood), m.instruments.clone(), listed(&m.voice), listed(&m.era), listed(&m.production)];
        for (slot, values) in scores.iter_mut().zip(fields) {
            for v in values {
                // Counted together whatever the case; shown as first written.
                let e = slot.entry(v.to_lowercase()).or_insert((0, v));
                e.0 += w;
            }
        }
    }
    let top = |slot: &HashMap<String, (i32, String)>, n: usize, at_least: i32| {
        let mut v: Vec<&(i32, String)> = slot.values().filter(|(s, _)| *s >= at_least).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        v.into_iter().take(n).map(|(_, name)| name.clone()).collect::<Vec<_>>()
    };
    let mut avoid: Vec<(i32, String)> = scores[0].values().chain(scores[2].values()).filter(|(s, _)| *s < 0).cloned().collect();
    avoid.sort();
    Taste {
        genres: top(&scores[0], 3, 1),
        moods: top(&scores[1], 2, 1),
        instruments: top(&scores[2], 4, 1),
        voice: top(&scores[3], 1, 2).pop(),
        era: top(&scores[4], 1, 2).pop(),
        production: top(&scores[5], 1, 2).pop(),
        avoid: avoid.into_iter().take(5).map(|(_, n)| n).collect(),
        songs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MusicSpec;

    #[test]
    fn taste_follows_likes_and_steers_from_dislikes() {
        let song = |genre: &str, inst: &[&str], fav: bool, bad: bool| MediaItem {
            kind: "audio".into(),
            op: "music".into(),
            favorite: fav,
            disliked: bad,
            music: Some(MusicSpec { genre: Some(genre.into()), instruments: inst.iter().map(|s| s.to_string()).collect(), voice: Some("male".into()), ..Default::default() }),
            ..Default::default()
        };
        let items = vec![
            song("Country, Folk", &["Acoustic guitar", "pedal steel"], true, false),
            song("country", &["acoustic guitar"], false, false),
            song("EDM", &["synth", "808"], false, true),
            song("Pop", &["piano"], false, false),
            MediaItem { kind: "audio".into(), op: "narrate".into(), ..Default::default() },
        ];
        let t = taste(&items);
        assert_eq!(t.songs, 4);
        assert_eq!(t.genres, vec!["Country", "Folk", "Pop"], "liked first, the disliked left out");
        assert_eq!(t.instruments[0], "Acoustic guitar");
        assert!(!t.instruments.contains(&"synth".to_string()));
        assert_eq!(t.voice.as_deref(), Some("male"));
        assert_eq!(t.avoid, vec!["808", "EDM", "synth"]);
        assert_eq!(taste(&[]), Taste::default());
    }

    #[test]
    fn playlists_keep_their_order_and_lose_deleted_songs() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::for_test();
        let road = save(&conn, &c, Playlist { name: " Road trip ".into(), songs: vec!["b".into(), "a".into(), "b".into()], ..Default::default() }).unwrap();
        assert_eq!((road.name.as_str(), road.songs.clone()), ("Road trip", vec!["b".to_string(), "a".to_string()]), "trimmed, each song once");
        assert!(save(&conn, &c, Playlist { name: "road TRIP".into(), ..Default::default() }).is_err(), "names are one each");
        assert!(save(&conn, &c, Playlist { name: "  ".into(), ..Default::default() }).is_err());
        let renamed = save(&conn, &c, Playlist { name: "Summer".into(), ..road.clone() }).unwrap();
        assert_eq!((renamed.id.as_str(), renamed.created_at), (road.id.as_str(), road.created_at));
        forget_song(&conn, &c, "b");
        assert_eq!(list(&conn, &c)[0].songs, vec!["a".to_string()]);
        remove(&conn, &c, &road.id).unwrap();
        assert!(list(&conn, &c).is_empty());
    }
}
