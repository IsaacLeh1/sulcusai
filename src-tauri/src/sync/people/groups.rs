// SPDX-License-Identifier: AGPL-3.0-only
//! Group letters between people over MLS (`p2p::mls_store`). The PC that
//! makes a group is its owner and the only one that adds or removes, so two
//! changes never race for the same epoch. Members are device ids, the same
//! ids People contacts carry, and every MLS message goes to each other
//! member over the People link (by mail when they're away).

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::link::{self, Frame};
use crate::crypto::Cipher;
use crate::db;
use crate::features::{self, Feature};
use crate::p2p::mls_store as mls;
use crate::{AppState, AppStateRef};
use sulcus_net::mls::Received;

const KEY: &str = "people.mls";
/// Letters kept per group; the oldest go first.
const KEEP: usize = 2000;
/// Messages that couldn't be opened yet (a change they depend on is still
/// on its way), kept to try again after the next change.
const HELD: usize = 200;
const MAX_TEXT: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub(super) enum Wire {
    Invite { group: String, name: String },
    KeyPackage { group: String, kp: String },
    Decline { group: String },
    Welcome { group: String, name: String, welcome: String },
    /// A letter or a change of members, as MLS sealed it.
    Msg { group: String, raw: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Book {
    groups: Vec<Rec>,
    invites: Vec<Invite>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rec {
    id: String,
    name: String,
    owner: String,
    letters: Vec<Letter>,
    /// People asked to join who haven't sent a key package yet (owner only).
    invited: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    held: Vec<String>,
    /// Removed by the owner: the keys are gone, the letters stay.
    removed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Letter {
    from: String,
    text: String,
    at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    group: String,
    name: String,
    from: String,
}

type Out = Vec<(String, Wire)>;

fn book(conn: &Connection) -> Book {
    db::get(conn, KEY).unwrap_or_default()
}

fn save(conn: &Connection, b: &Book) -> Result<(), String> {
    db::set(conn, KEY, b)
}

fn rec<'a>(b: &'a mut Book, group: &str) -> Result<&'a mut Rec, String> {
    b.groups.iter_mut().find(|r| r.id == group).ok_or_else(|| "That group isn't on this PC.".to_string())
}

fn others(conn: &Connection, c: &Cipher, group: &str, me: &str) -> Result<Vec<String>, String> {
    Ok(mls::members(conn, c, group)?.into_iter().filter(|m| m != me).collect())
}

fn to_all(to: Vec<String>, w: Wire) -> Out {
    to.into_iter().map(|t| (t, w.clone())).collect()
}

pub(super) fn create(conn: &Connection, c: &Cipher, me: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the group a name.".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    mls::create(conn, c, &id, me)?;
    let mut b = book(conn);
    b.groups.push(Rec { id: id.clone(), name: name.chars().take(80).collect(), owner: me.into(), ..Default::default() });
    save(conn, &b)?;
    Ok(id)
}

pub(super) fn invite(conn: &Connection, me: &str, group: &str, who: &str) -> Result<Out, String> {
    let mut b = book(conn);
    let r = rec(&mut b, group)?;
    if r.owner != me || r.removed {
        return Err("Only the person who made the group adds people.".into());
    }
    if !r.invited.iter().any(|i| i == who) {
        r.invited.push(who.into());
    }
    let w = Wire::Invite { group: group.into(), name: r.name.clone() };
    save(conn, &b)?;
    Ok(vec![(who.into(), w)])
}

/// Answers an invitation with a key package; the owner's welcome follows.
pub(super) fn accept(conn: &Connection, c: &Cipher, me: &str, group: &str) -> Result<Out, String> {
    let mut b = book(conn);
    let inv = b.invites.iter().find(|i| i.group == group).cloned().ok_or("That invitation isn't here anymore.")?;
    let kp = mls::key_package(conn, c, group, me)?;
    b.invites.retain(|i| i.group != group);
    save(conn, &b)?;
    Ok(vec![(inv.from, Wire::KeyPackage { group: group.into(), kp: B64.encode(kp) })])
}

pub(super) fn decline(conn: &Connection, group: &str) -> Result<Out, String> {
    let mut b = book(conn);
    let Some(inv) = b.invites.iter().find(|i| i.group == group).cloned() else { return Ok(vec![]) };
    b.invites.retain(|i| i.group != group);
    save(conn, &b)?;
    Ok(vec![(inv.from, Wire::Decline { group: group.into() })])
}

/// Removes a member; the commit re-keys the group, and they get it too so
/// their PC knows it's out.
pub(super) fn remove(conn: &Connection, c: &Cipher, me: &str, group: &str, who: &str) -> Result<Out, String> {
    let mut b = book(conn);
    let r = rec(&mut b, group)?;
    if r.owner != me {
        return Err("Only the person who made the group removes people.".into());
    }
    if who == me {
        return Err("The group's maker can't remove themselves.".into());
    }
    r.invited.retain(|i| i != who);
    save(conn, &b)?;
    let to = others(conn, c, group, me)?;
    let commit = mls::remove(conn, c, group, who)?;
    Ok(to_all(to, Wire::Msg { group: group.into(), raw: B64.encode(commit) }))
}

pub(super) fn send(conn: &Connection, c: &Cipher, me: &str, group: &str, text: &str, at: i64) -> Result<Out, String> {
    let text: String = text.trim().chars().take(MAX_TEXT).collect();
    if text.is_empty() {
        return Ok(vec![]);
    }
    let mut b = book(conn);
    if rec(&mut b, group)?.removed {
        return Err("You were removed from that group.".into());
    }
    let to = others(conn, c, group, me)?;
    let raw = mls::send(conn, c, group, text.as_bytes())?;
    keep(rec(&mut b, group)?, Letter { from: me.into(), text, at });
    save(conn, &b)?;
    Ok(to_all(to, Wire::Msg { group: group.into(), raw: B64.encode(raw) }))
}

/// Forgets a group here (its keys and letters).
pub(super) fn leave(conn: &Connection, group: &str) -> Result<(), String> {
    let mut b = book(conn);
    b.groups.retain(|r| r.id != group);
    save(conn, &b)?;
    if mls::has(conn, group) {
        mls::forget(conn, group)?;
    }
    Ok(())
}

fn keep(r: &mut Rec, l: Letter) {
    r.letters.push(l);
    if r.letters.len() > KEEP {
        let cut = r.letters.len() - KEEP;
        r.letters.drain(..cut);
    }
}

/// One MLS message for a group this PC is in. A message that can't be
/// opened yet is held; a replay is dropped.
fn take(conn: &Connection, c: &Cipher, me: &str, r: &mut Rec, raw: &[u8], at: i64) -> Result<bool, String> {
    match mls::receive(conn, c, &r.id, raw) {
        Ok(Received::Letter { from, body }) => {
            keep(r, Letter { from, text: String::from_utf8_lossy(&body).into_owned(), at });
            Ok(true)
        }
        Ok(Received::Changed) => {
            if !mls::members(conn, c, &r.id)?.iter().any(|m| m == me) {
                r.removed = true;
                r.held.clear();
                mls::forget(conn, &r.id)?;
            }
            Ok(true)
        }
        Err(e) if e.contains("already opened") => Ok(false),
        Err(_) => {
            let s = B64.encode(raw);
            if !r.held.contains(&s) {
                r.held.push(s);
                if r.held.len() > HELD {
                    r.held.remove(0);
                }
            }
            Ok(false)
        }
    }
}

/// What came from `from` over the People link; returns what to send on.
pub(super) fn handle(conn: &Connection, c: &Cipher, me: &str, from: &str, w: Wire, at: i64) -> Result<Out, String> {
    let mut b = book(conn);
    let mut out = Vec::new();
    match w {
        Wire::Invite { group, name } => {
            if !b.groups.iter().any(|r| r.id == group) && !b.invites.iter().any(|i| i.group == group) {
                b.invites.push(Invite { group, name: name.chars().take(80).collect(), from: from.into() });
            }
        }
        Wire::Decline { group } => rec(&mut b, &group)?.invited.retain(|i| i != from),
        Wire::KeyPackage { group, kp } => {
            let r = rec(&mut b, &group)?;
            if r.owner != me || !r.invited.iter().any(|i| i == from) {
                return Err("No invitation to that group went to them.".into());
            }
            let kp = B64.decode(kp).map_err(|_| "A damaged key package.")?;
            let to = others(conn, c, &group, me)?;
            let (commit, welcome) = mls::add(conn, c, &group, &kp)?;
            r.invited.retain(|i| i != from);
            out = to_all(to, Wire::Msg { group: group.clone(), raw: B64.encode(commit) });
            out.push((from.into(), Wire::Welcome { group, name: r.name.clone(), welcome: B64.encode(welcome) }));
        }
        Wire::Welcome { group, name, welcome } => {
            if b.groups.iter().any(|r| r.id == group) {
                return Ok(vec![]);
            }
            let welcome = B64.decode(welcome).map_err(|_| "A damaged welcome.")?;
            // Only answers this PC's own key package: join fails otherwise.
            mls::join(conn, c, &group, &welcome)?;
            b.groups.push(Rec { id: group, name: name.chars().take(80).collect(), owner: from.into(), ..Default::default() });
        }
        Wire::Msg { group, raw } => {
            let Ok(r) = rec(&mut b, &group) else { return Ok(vec![]) };
            if r.removed {
                return Ok(vec![]);
            }
            let raw = B64.decode(raw).map_err(|_| "A damaged group letter.")?;
            if take(conn, c, me, r, &raw, at)? {
                // Something moved on: try what was held until nothing more opens.
                loop {
                    let held = std::mem::take(&mut r.held);
                    let mut opened = false;
                    for h in held {
                        let Ok(raw) = B64.decode(&h) else { continue };
                        if r.removed {
                            break;
                        }
                        opened |= take(conn, c, me, r, &raw, at)?;
                    }
                    if !opened {
                        break;
                    }
                }
            }
        }
    }
    save(conn, &b)?;
    Ok(out)
}

// ---- The app: the People link and the window ----

fn me(state: &AppState) -> String {
    crate::sync::device_id(&state.paths.data)
}

fn dispatch(out: Out) {
    for (to, w) in out {
        link::send(&to, Frame::Mls { data: serde_json::to_string(&w).expect("wire json") });
    }
}

fn changed(state: &AppState) {
    super::emit(state, "people:groups", serde_json::Value::Null);
}

/// A group message from a contact over the link.
pub(super) fn incoming(state: &Arc<AppState>, from: &str, data: &str) {
    let Ok(w) = serde_json::from_str::<Wire>(data) else { return };
    let Ok(c) = state.work_cipher() else { return };
    let out = {
        let conn = state.db.lock().unwrap();
        if !features::is_on(&conn, Feature::People) {
            return;
        }
        handle(&conn, &c, &me(state), from, w, db::now_ms())
    };
    if let Ok(out) = out {
        dispatch(out);
    }
    changed(state);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    #[serde(flatten)]
    rec: Rec,
    members: Vec<String>,
    mine: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupsView {
    me: String,
    groups: Vec<GroupView>,
    invites: Vec<Invite>,
}

fn run(state: &AppState, f: impl FnOnce(&Connection, &Cipher, &str) -> Result<Out, String>) -> Result<(), String> {
    features::require(state, Feature::People)?;
    let c = state.cipher()?;
    let out = f(&state.db.lock().unwrap(), &c, &me(state))?;
    dispatch(out);
    link::wake();
    changed(state);
    Ok(())
}

#[tauri::command(async)]
pub fn people_groups(state: AppStateRef) -> Result<GroupsView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let me = me(&state);
    let conn = state.db.lock().unwrap();
    let b = book(&conn);
    let groups = b
        .groups
        .into_iter()
        .map(|r| GroupView { members: if r.removed { vec![] } else { mls::members(&conn, &c, &r.id).unwrap_or_default() }, mine: r.owner == me, rec: r })
        .collect();
    Ok(GroupsView { me, groups, invites: b.invites })
}

#[tauri::command(async)]
pub fn people_group_create(state: AppStateRef, name: String) -> Result<String, String> {
    let mut id = String::new();
    run(&state, |conn, c, me| {
        id = create(conn, c, me, &name)?;
        Ok(vec![])
    })?;
    Ok(id)
}

#[tauri::command(async)]
pub fn people_group_invite(state: AppStateRef, group: String, id: String) -> Result<(), String> {
    run(&state, |conn, _, me| invite(conn, me, &group, &id))
}

#[tauri::command(async)]
pub fn people_group_answer(state: AppStateRef, group: String, accept: bool) -> Result<(), String> {
    run(&state, |conn, c, me| if accept { self::accept(conn, c, me, &group) } else { decline(conn, &group) })
}

#[tauri::command(async)]
pub fn people_group_remove(state: AppStateRef, group: String, member: String) -> Result<(), String> {
    run(&state, |conn, c, me| remove(conn, c, me, &group, &member))
}

#[tauri::command(async)]
pub fn people_group_send(state: AppStateRef, group: String, text: String) -> Result<(), String> {
    run(&state, |conn, c, me| send(conn, c, me, &group, &text, db::now_ms()))
}

#[tauri::command(async)]
pub fn people_group_leave(state: AppStateRef, group: String) -> Result<(), String> {
    run(&state, |conn, _, _| leave(conn, &group).map(|_| vec![]))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pc {
        id: &'static str,
        conn: Connection,
    }

    fn pc(id: &'static str) -> Pc {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        Pc { id, conn }
    }

    /// Delivers everything in `out` (and what it causes) between the PCs.
    fn deliver(pcs: &[&Pc], c: &Cipher, from: &str, out: Out) {
        let mut queue: Vec<(String, String, Wire)> = out.into_iter().map(|(to, w)| (from.to_string(), to, w)).collect();
        while !queue.is_empty() {
            let (from, to, w) = queue.remove(0);
            let p = pcs.iter().find(|p| p.id == to).unwrap();
            for (next, w) in handle(&p.conn, c, p.id, &from, w, 1).unwrap() {
                queue.push((p.id.to_string(), next, w));
            }
        }
    }

    fn texts(p: &Pc, g: &str) -> Vec<String> {
        book(&p.conn).groups.into_iter().find(|r| r.id == g).unwrap().letters.into_iter().map(|l| format!("{}:{}", l.from, l.text)).collect()
    }

    fn join(pcs: &[&Pc], c: &Cipher, owner: &Pc, g: &str, who: &Pc) {
        deliver(pcs, c, owner.id, invite(&owner.conn, owner.id, g, who.id).unwrap());
        deliver(pcs, c, who.id, accept(&who.conn, c, who.id, g).unwrap());
    }

    #[test]
    fn three_members_write_add_and_remove() {
        let c = Cipher::for_test();
        let (ana, bo, cy) = (pc("ana"), pc("bo"), pc("cy"));
        let all = [&ana, &bo, &cy];
        let g = create(&ana.conn, &c, "ana", "Family").unwrap();
        join(&all, &c, &ana, &g, &bo);
        join(&all, &c, &ana, &g, &cy);
        assert_eq!(mls::members(&cy.conn, &c, &g).unwrap().len(), 3);
        assert_eq!(book(&cy.conn).groups[0].owner, "ana");

        deliver(&all, &c, "bo", send(&bo.conn, &c, "bo", &g, "hi all", 2).unwrap());
        deliver(&all, &c, "cy", send(&cy.conn, &c, "cy", &g, "hello", 3).unwrap());
        assert_eq!(texts(&ana, &g), ["bo:hi all", "cy:hello"]);
        assert_eq!(texts(&bo, &g), ["bo:hi all", "cy:hello"]);

        // Only the owner changes members.
        assert!(remove(&bo.conn, &c, "bo", &g, "cy").is_err());
        deliver(&all, &c, "ana", remove(&ana.conn, &c, "ana", &g, "cy").unwrap());
        assert!(book(&cy.conn).groups[0].removed);
        assert!(!mls::has(&cy.conn, &g), "the removed PC's keys are gone");
        let after = send(&ana.conn, &c, "ana", &g, "just us", 4).unwrap();
        assert_eq!(after.len(), 1, "the removed member gets nothing");
        deliver(&all, &c, "ana", after);
        assert_eq!(texts(&bo, &g).last().unwrap(), "ana:just us");
        assert!(send(&cy.conn, &c, "cy", &g, "still here?", 5).is_err());
    }

    #[test]
    fn out_of_order_and_replayed_messages() {
        let c = Cipher::for_test();
        let (ana, bo, cy) = (pc("ana"), pc("bo"), pc("cy"));
        let all = [&ana, &bo, &cy];
        let g = create(&ana.conn, &c, "ana", "Club").unwrap();
        join(&all, &c, &ana, &g, &bo);

        // Cy joins; Bo gets ana's next letter before the commit adding Cy.
        deliver(&all, &c, "ana", invite(&ana.conn, "ana", &g, "cy").unwrap());
        let added = handle(&ana.conn, &c, "ana", "cy", Wire::KeyPackage { group: g.clone(), kp: match &accept(&cy.conn, &c, "cy", &g).unwrap()[0].1 { Wire::KeyPackage { kp, .. } => kp.clone(), _ => unreachable!() } }, 1).unwrap();
        let letter = send(&ana.conn, &c, "ana", &g, "welcome cy", 2).unwrap();
        let to_bo = |out: &Out| out.iter().find(|(t, _)| t == "bo").unwrap().1.clone();
        handle(&bo.conn, &c, "bo", "ana", to_bo(&letter), 3).unwrap();
        assert!(texts(&bo, &g).is_empty(), "held until the change arrives");
        handle(&bo.conn, &c, "bo", "ana", to_bo(&added), 3).unwrap();
        assert_eq!(texts(&bo, &g), ["ana:welcome cy"]);

        // The same letter again changes nothing.
        handle(&bo.conn, &c, "bo", "ana", to_bo(&letter), 4).unwrap();
        assert_eq!(texts(&bo, &g).len(), 1);

        // Cy's welcome, then two letters in the wrong order.
        deliver(&all, &c, "ana", added.into_iter().filter(|(t, _)| t == "cy").collect());
        let one = send(&bo.conn, &c, "bo", &g, "one", 5).unwrap();
        let two = send(&bo.conn, &c, "bo", &g, "two", 6).unwrap();
        deliver(&all, &c, "bo", two.into_iter().filter(|(t, _)| t == "cy").collect());
        deliver(&all, &c, "bo", one.into_iter().filter(|(t, _)| t == "cy").collect());
        assert_eq!(texts(&cy, &g), ["bo:two", "bo:one"]);
    }

    #[test]
    fn welcomes_and_key_packages_need_an_invitation() {
        let c = Cipher::for_test();
        let (ana, bo) = (pc("ana"), pc("bo"));
        let g = create(&ana.conn, &c, "ana", "G").unwrap();
        let kp = B64.encode(mls::key_package(&bo.conn, &c, "other", "bo").unwrap());
        assert!(handle(&ana.conn, &c, "ana", "bo", Wire::KeyPackage { group: g.clone(), kp }, 1).is_err());
        assert!(accept(&bo.conn, &c, "bo", &g).is_err());
        deliver(&[&ana, &bo], &c, "ana", invite(&ana.conn, "ana", &g, "bo").unwrap());
        deliver(&[&ana, &bo], &c, "bo", decline(&bo.conn, &g).unwrap());
        assert!(book(&ana.conn).groups[0].invited.is_empty());
        assert!(book(&bo.conn).invites.is_empty());
    }
}
