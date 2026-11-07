// SPDX-License-Identifier: AGPL-3.0-only
//! Profiles: each user writes their own, kept on their PC and sent peer to
//! peer to the people they share a group with. Nothing goes to a server.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use super::store::{book, me, send, Wire};
use super::MemberId;
use crate::{db, AppState, AppStateRef};

pub(super) const MINE: &str = "social_profile";
const THEIRS: &str = "social_profiles";
/// Base64 characters: pictures travel to every member, so they stay small.
pub const MAX_PICTURE: usize = 350_000;
pub const MAX_BANNER: usize = 700_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    /// Shown to others as offline.
    Invisible,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// data: URLs of small pictures.
    pub picture: String,
    pub banner: String,
    pub about: String,
    pub pronouns: String,
    pub presence: Presence,
    pub status: String,
    pub links: Vec<String>,
    /// Extras: the profile's two theme colours ("#rrggbb"), shown as a gradient.
    pub accent: String,
    pub accent2: String,
    /// Extras: a drawn frame around the picture ("glow", "gold", "rainbow", "dashed").
    pub frame: String,
    /// Extras: the frame's colour ("#rrggbb"); empty is the frame's own.
    pub frame_color: String,
    /// Extras: the font this person's messages show in ("serif", "mono", "rounded", "hand", "script", "narrow").
    pub font: String,
    /// Extras: the colour this person's message text shows in; empty is the theme's.
    pub text_color: String,
    /// Extras: a different name in some groups, by group id.
    pub nicknames: std::collections::BTreeMap<String, String>,
    /// Others see this person as offline except while they're in a voice channel.
    pub voice_only: bool,
    /// Set by the app on joining and leaving voice, for `voice_only`.
    pub in_voice: bool,
    /// When it was last changed, so an older copy never replaces a newer one.
    pub at: i64,
}

fn picture_ok(p: &str, max: usize) -> bool {
    p.is_empty() || (p.len() <= max && ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,", "data:image/gif;base64,"].iter().any(|h| p.starts_with(h)))
}

/// Checks a profile and trims what's too long.
pub fn checked(mut p: Profile) -> Result<Profile, String> {
    let cut = |s: &str, n: usize| s.trim().chars().take(n).collect::<String>();
    p.name = cut(&p.name, 40);
    p.about = cut(&p.about, 500);
    p.pronouns = cut(&p.pronouns, 40);
    p.status = cut(&p.status, 100);
    p.links = p.links.iter().map(|l| cut(l, 300)).filter(|l| l.starts_with("https://") || l.starts_with("http://")).take(5).collect();
    let colour = |c: &str| c.is_empty() || (c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()));
    if !["", "serif", "mono", "rounded", "hand", "script", "narrow"].contains(&p.font.as_str()) {
        p.font.clear();
    }
    if !colour(&p.accent) || !colour(&p.accent2) || !colour(&p.frame_color) || !colour(&p.text_color) {
        return Err("Profile colours are like #5865f2.".into());
    }
    if !["", "glow", "gold", "rainbow", "dashed"].contains(&p.frame.as_str()) {
        p.frame.clear();
    }
    p.nicknames = p.nicknames.iter().filter(|(g, n)| g.len() <= 64 && !n.trim().is_empty()).take(100).map(|(g, n)| (g.clone(), cut(n, 32))).collect();
    if !picture_ok(&p.picture, MAX_PICTURE) || !picture_ok(&p.banner, MAX_BANNER) {
        return Err("Profile pictures are PNG, JPEG, WebP or GIF, about 250 KB at most (banners 500 KB).".into());
    }
    Ok(p)
}

/// Everyone this PC shares a group with.
fn people(state: &AppState) -> Vec<MemberId> {
    let mine = me(state);
    let b = book(&state.db.lock().unwrap());
    let mut out: Vec<MemberId> = b.groups.iter().flat_map(|r| r.group.members.keys().cloned()).filter(|m| *m != mine && !b.personal.blocked.contains(m)).collect();
    out.sort();
    out.dedup();
    out
}

/// As others see it: invisible reads as offline.
fn shared(p: &Profile) -> Profile {
    let mut out = p.clone();
    if out.voice_only && !out.in_voice {
        out.presence = Presence::Invisible;
    }
    if out.presence == Presence::Invisible {
        out.status.clear();
    }
    out
}

pub(super) fn profile_in(state: &Arc<AppState>, from: &str, p: Profile) -> Result<(), String> {
    if !people(state).iter().any(|m| m == from) {
        return Ok(());
    }
    let p = checked(p)?;
    {
        let conn = state.db.lock().unwrap();
        let mut all: BTreeMap<MemberId, Profile> = db::get(&conn, THEIRS).unwrap_or_default();
        if all.get(from).is_some_and(|old| old.at >= p.at) {
            return Ok(());
        }
        all.insert(from.into(), p);
        db::set(&conn, THEIRS, &all)?;
    }
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:profile", json!({ "from": from }));
    }
    Ok(())
}

#[tauri::command]
pub async fn social_profile(state: AppStateRef<'_>) -> Result<Profile, String> {
    Ok(db::get(&state.db.lock().unwrap(), MINE).unwrap_or_default())
}

/// Saves this user's profile and sends it to the people they share a group with.
#[tauri::command]
pub async fn social_set_profile(state: AppStateRef<'_>, profile: Profile) -> Result<usize, String> {
    let mut p = checked(profile)?;
    p.at = super::now_ms();
    db::set(&state.db.lock().unwrap(), MINE, &p)?;
    // Shown at once wherever this user appears on this PC.
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:profile", json!({ "from": me(&state) }));
    }
    let to = people(&state);
    let w = Wire::Profile { profile: shared(&p) };
    Ok(to.iter().filter(|m| send(m, &w)).count())
}

/// Profiles by member id: other people's and this user's own.
#[tauri::command]
pub async fn social_profiles(state: AppStateRef<'_>) -> Result<BTreeMap<MemberId, Profile>, String> {
    let conn = state.db.lock().unwrap();
    let mut all: BTreeMap<MemberId, Profile> = db::get(&conn, THEIRS).unwrap_or_default();
    // This user's own too, so their name, picture and colours show as set.
    if let Some(mine) = db::get::<Profile>(&conn, MINE) {
        all.insert(me(&state), mine);
    }
    Ok(all)
}
