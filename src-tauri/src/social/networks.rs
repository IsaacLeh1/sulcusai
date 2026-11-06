// SPDX-License-Identifier: AGPL-3.0-only
//! Networks in Social: virtual LANs (p2p::sharing) that belong to a group
//! the way channels do. Admins list them to the group, people ask to join
//! through their Social link, and an admin's PC answers by the network's
//! rules (invite, password, approval). The member list itself still travels
//! signed between members (sulcus_net::sharing::roster).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sulcus_net::sharing::vlan::{Join, Network};
use tauri::Emitter;

use super::store::{book, me, send, Wire};
use super::MemberId;
use crate::p2p::sharing::{lan_on, my_peer_id, networks, push_roster, update_networks};
use crate::{db, AppState, AppStateRef};

const CARDS: &str = "social_net_cards";

/// What a group's members see of a network before they're in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub group: String,
    pub has_password: bool,
    pub approval: bool,
    pub members: usize,
    /// Social devices of the admins, where join requests go.
    pub admins: Vec<MemberId>,
}

pub fn card(n: &Network) -> Option<Card> {
    Some(Card {
        id: n.id.clone(),
        name: n.name.clone(),
        group: n.group.clone()?,
        has_password: n.has_password(),
        approval: n.approval,
        members: n.members.len(),
        admins: n.admins.iter().filter_map(|a| n.devices.get(a).cloned()).collect(),
    })
}

/// Between PCs, inside the Social link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "n", rename_all = "snake_case")]
pub enum NetWire {
    /// The networks of a group, from one of their admins.
    Cards { group: String, cards: Vec<Card> },
    /// Asking to join; `peer` is the asker's network id.
    Ask { net: String, peer: String, name: String, password: Option<String>, invite: Option<String> },
    /// The answer to an ask.
    Answer { net: String, text: String },
    /// An invite sent to one person.
    Invite { card: Card, code: String },
}

fn emit(state: &AppState) {
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:networks", ());
    }
}

fn cards(state: &AppState) -> BTreeMap<String, Vec<Card>> {
    db::get(&state.db.lock().unwrap(), CARDS).unwrap_or_default()
}

/// Members of `group` on this PC's list, to send cards to.
fn group_devices(state: &AppState, group: &str) -> Vec<MemberId> {
    let mine = me(state);
    book(&state.db.lock().unwrap()).groups.iter().find(|r| r.group.id == group).map(|r| r.group.members.keys().filter(|m| **m != mine).cloned().collect()).unwrap_or_default()
}

/// Sends the group's networks (the ones this PC admins) to the group.
fn announce(state: &AppState, group: &str) {
    let Ok(nets) = networks(state) else { return };
    let mine: Vec<Card> = nets.iter().filter(|n| n.group.as_deref() == Some(group)).filter_map(card).collect();
    let w = Wire::Net { msg: NetWire::Cards { group: group.into(), cards: mine.clone() } };
    for m in group_devices(state, group) {
        send(&m, &w);
    }
    let conn = state.db.lock().unwrap();
    let mut all: BTreeMap<String, Vec<Card>> = db::get(&conn, CARDS).unwrap_or_default();
    let list = all.entry(group.into()).or_default();
    list.retain(|c| !mine.iter().any(|x| x.id == c.id) && nets.iter().all(|n| n.id != c.id || n.group.as_deref() == Some(group)));
    list.extend(mine);
    let _ = db::set(&conn, CARDS, &all);
}

fn in_group(state: &AppState, group: &str, who: &str) -> bool {
    book(&state.db.lock().unwrap()).groups.iter().any(|r| r.group.id == group && r.group.members.contains_key(who))
}

/// Social traffic about networks from another PC.
pub(super) fn net_in(state: &Arc<AppState>, from: &str, msg: NetWire) -> Result<(), String> {
    match msg {
        NetWire::Cards { group, cards: list } => {
            if !in_group(state, &group, from) {
                return Ok(());
            }
            // Only what that PC admins, for that group.
            let list: Vec<Card> = list.into_iter().filter(|c| c.group == group && c.admins.iter().any(|a| a == from)).collect();
            let conn = state.db.lock().unwrap();
            let mut all: BTreeMap<String, Vec<Card>> = db::get(&conn, CARDS).unwrap_or_default();
            let g = all.entry(group).or_default();
            g.retain(|c| !c.admins.iter().any(|a| a == from));
            g.extend(list);
            db::set(&conn, CARDS, &all)?;
        }
        NetWire::Ask { net, peer, name, password, invite } => {
            let mine = my_peer_id(state)?;
            let reply = |text: &str| send(from, &Wire::Net { msg: NetWire::Answer { net: net.clone(), text: text.into() } });
            let r = update_networks(state, |nets| {
                let n = nets.iter_mut().find(|n| n.id == net).ok_or("That network isn't on this PC.")?;
                // A group's networks take only that group's members, unless invited.
                if let Some(g) = &n.group {
                    if invite.is_none() && !in_group(state, g, from) {
                        return Err("Ask to join the group first.".into());
                    }
                }
                let out = n.join(&mine, &peer, password.as_deref(), invite.as_deref()).map_err(|e| e.message().to_string())?;
                n.names.insert(peer.clone(), name.chars().take(40).collect());
                n.devices.insert(peer.clone(), from.to_string());
                if matches!(out, Join::Admitted(_)) {
                    n.version += 1;
                }
                Ok((out, n.clone()))
            });
            match r {
                Ok((Join::Admitted(a), n)) => {
                    push_roster(state, &n, std::slice::from_ref(&peer));
                    reply(&format!("You're in {}: your address is {a}.", n.name));
                }
                Ok((Join::Waiting, n)) => {
                    reply(&format!("Asked to join {}. An admin will let you in.", n.name));
                }
                Err(e) => {
                    reply(&e);
                }
            }
            emit(state);
        }
        NetWire::Answer { net, text } => {
            if let Some(app) = state.app.get() {
                let _ = app.emit("social:network_answer", json!({ "net": net, "text": text }));
            }
        }
        NetWire::Invite { card, code } => {
            let conn = state.db.lock().unwrap();
            let mut invites: Vec<(Card, String)> = db::get(&conn, "social_net_invites").unwrap_or_default();
            invites.retain(|(c, _)| c.id != card.id);
            // Only an admin of the network may invite.
            if card.admins.iter().any(|a| a == from) {
                invites.push((card, code));
            }
            db::set(&conn, "social_net_invites", &invites)?;
        }
    }
    emit(state);
    Ok(())
}

// ---------- the view ----------

#[derive(Debug, Clone, Serialize)]
pub struct MemberRow {
    pub peer: String,
    pub name: String,
    pub address: String,
    pub admin: bool,
    pub me: bool,
    pub online: bool,
    /// Round trip in milliseconds, when connected.
    pub ping: Option<u64>,
    /// Their Social device, for chat and calls.
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetView {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub admin: bool,
    pub owner: bool,
    pub connected: bool,
    pub has_password: bool,
    pub approval: bool,
    pub limit: usize,
    pub members: Vec<MemberRow>,
    pub waiting: Vec<(String, String)>,
    pub banned: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetsView {
    pub lan_on: bool,
    pub mine: Vec<NetView>,
    /// Networks in this PC's groups it isn't in yet.
    pub open: Vec<Card>,
    pub invites: Vec<(Card, String)>,
    pub my_address: Option<String>,
}

pub fn rows(n: &Network, me: &str, online: &dyn Fn(&str) -> (bool, Option<u64>)) -> Vec<MemberRow> {
    let mut out: Vec<MemberRow> = n
        .members
        .iter()
        .map(|(p, a)| {
            let (on, ping) = if p == me { (true, Some(0)) } else { online(p) };
            MemberRow { peer: p.clone(), name: n.names.get(p).cloned().unwrap_or_else(|| p.chars().take(8).collect()), address: a.to_string(), admin: n.admins.contains(p), me: p == me, online: on, ping, device: n.devices.get(p).cloned() }
        })
        .collect();
    // Online first, then by name, like the list a game host reads.
    out.sort_by(|a, b| b.me.cmp(&a.me).then(b.online.cmp(&a.online)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

fn view(state: &AppState) -> Result<NetsView, String> {
    let lan = lan_on(state).is_ok();
    let me_peer = my_peer_id(state).unwrap_or_default();
    let nets = networks(state).unwrap_or_default();
    let node = crate::p2p::running();
    let online = |p: &str| -> (bool, Option<u64>) {
        let Some(node) = &node else { return (false, None) };
        let Ok(id) = p.parse() else { return (false, None) };
        let on = node.connected().contains(&id);
        (on, node.rtt(&id).filter(|_| on).map(|d| d.as_millis() as u64))
    };
    let up = crate::p2p::vlan_adapter::running();
    let name_of = |n: &Network, p: &String| (p.clone(), n.names.get(p).cloned().unwrap_or_else(|| p.chars().take(8).collect()));
    let mine: Vec<NetView> = nets
        .iter()
        .map(|n| NetView {
            id: n.id.clone(),
            name: n.name.clone(),
            group: n.group.clone(),
            admin: n.admins.contains(&me_peer),
            owner: n.admins.iter().next() == Some(&me_peer),
            connected: up.as_deref() == Some(n.id.as_str()),
            has_password: n.has_password(),
            approval: n.approval,
            limit: n.limit,
            members: rows(n, &me_peer, &online),
            waiting: n.waiting.iter().map(|p| name_of(n, p)).collect(),
            banned: n.banned.iter().map(|p| name_of(n, p)).collect(),
        })
        .collect();
    let open: Vec<Card> = cards(state).into_values().flatten().filter(|c| !nets.iter().any(|n| n.id == c.id)).collect();
    let invites: Vec<(Card, String)> = db::get::<Vec<(Card, String)>>(&state.db.lock().unwrap(), "social_net_invites").unwrap_or_default().into_iter().filter(|(c, _)| !nets.iter().any(|n| n.id == c.id)).collect();
    let my_address = up.and_then(|id| nets.iter().find(|n| n.id == id).and_then(|n| n.members.get(&me_peer)).map(|a| a.to_string()));
    Ok(NetsView { lan_on: lan, mine, open, invites, my_address })
}

#[tauri::command]
pub fn social_networks(state: AppStateRef) -> Result<NetsView, String> {
    view(&state)
}

fn my_name(state: &AppState) -> String {
    let p: super::profile::Profile = db::get(&state.db.lock().unwrap(), "social_profile").unwrap_or_default();
    if p.name.is_empty() { "Someone".into() } else { p.name }
}

#[tauri::command]
pub fn social_net_create(state: AppStateRef, group: Option<String>, name: String, password: String, approval: bool, limit: usize) -> Result<NetsView, String> {
    lan_on(&state)?;
    let me_peer = my_peer_id(&state)?;
    if name.trim().is_empty() {
        return Err("Give the network a name.".into());
    }
    let id = hex::encode(sulcus_net::sharing::share::new_key())[..16].to_string();
    let mut n = Network::create(&id, name.trim(), &me_peer);
    n.group = group.clone();
    n.names.insert(me_peer.clone(), my_name(&state));
    n.devices.insert(me_peer.clone(), me(&state));
    n.set_rules(&me_peer, Some(&password), approval, limit).map_err(|e| e.message())?;
    update_networks(&state, |nets| {
        nets.push(n);
        Ok(())
    })?;
    if let Some(g) = &group {
        announce(&state, g);
    }
    view(&state)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Manage {
    Rename { name: String },
    Rules { password: Option<String>, approval: bool, limit: usize },
    Kick { peer: String },
    Ban { peer: String },
    Unban { peer: String },
    Admit { peer: String },
    Deny { peer: String },
    Admin { peer: String, on: bool },
    Leave,
    Delete,
}

#[tauri::command]
pub fn social_net_manage(state: AppStateRef, id: String, op: Manage) -> Result<NetsView, String> {
    let me_peer = my_peer_id(&state)?;
    let m = |e: sulcus_net::sharing::vlan::Refused| e.message().to_string();
    let (n, also, gone) = update_networks(&state, |nets| {
        let i = nets.iter().position(|n| n.id == id).ok_or("Unknown network.")?;
        let n = &mut nets[i];
        let mut also = vec![];
        match &op {
            Manage::Rename { name } => n.rename(&me_peer, name).map_err(m)?,
            Manage::Rules { password, approval, limit } => n.set_rules(&me_peer, password.as_deref(), *approval, *limit).map_err(m)?,
            Manage::Kick { peer } => {
                n.remove(&me_peer, peer).map_err(m)?;
                also.push(peer.clone());
            }
            Manage::Ban { peer } => {
                n.ban(&me_peer, peer).map_err(m)?;
                also.push(peer.clone());
            }
            Manage::Unban { peer } => n.unban(&me_peer, peer).map_err(m)?,
            Manage::Admit { peer } => {
                n.admit(&me_peer, peer).map_err(m)?;
                also.push(peer.clone());
            }
            Manage::Deny { peer } => n.deny(&me_peer, peer).map_err(m)?,
            Manage::Admin { peer, on: true } => n.make_admin(&me_peer, peer).map_err(m)?,
            Manage::Admin { peer, on: false } => n.demote(&me_peer, peer).map_err(m)?,
            Manage::Leave => {
                let old = n.clone();
                n.remove(&me_peer, &me_peer).map_err(m)?;
                let out = n.clone();
                nets.remove(i);
                // Only an admin's change reaches the others.
                return Ok((out, if old.admins.contains(&me_peer) { old.members.keys().cloned().collect() } else { vec![] }, true));
            }
            Manage::Delete => {
                if n.admins.iter().next() != Some(&me_peer) {
                    return Err("Only the network's owner can delete it.".into());
                }
                let everyone: Vec<String> = n.members.keys().cloned().collect();
                n.members.clear();
                n.admins.clear();
                n.version += 1;
                let out = n.clone();
                nets.remove(i);
                return Ok((out, everyone, true));
            }
        }
        Ok((n.clone(), also, false))
    })?;
    if gone && crate::p2p::vlan_adapter::running().as_deref() == Some(id.as_str()) {
        crate::p2p::vlan_adapter::stop();
    }
    push_roster(&state, &n, &also);
    if let Some(g) = &n.group {
        announce(&state, g);
    }
    view(&state)
}

/// A code that lets the holder in without the password, and a link with it.
#[tauri::command]
pub fn social_net_invite(state: AppStateRef, id: String, to: Option<MemberId>) -> Result<String, String> {
    let me_peer = my_peer_id(&state)?;
    let code = super::groups::new_invite_code();
    let n = update_networks(&state, |nets| {
        let n = nets.iter_mut().find(|n| n.id == id).ok_or("Unknown network.")?;
        n.add_invite(&me_peer, &code).map_err(|e| e.message().to_string())?;
        Ok(n.clone())
    })?;
    push_roster(&state, &n, &[]);
    let mut c = card(&n).unwrap_or(Card { id: n.id.clone(), name: n.name.clone(), group: String::new(), has_password: n.has_password(), approval: n.approval, members: n.members.len(), admins: vec![] });
    if c.admins.is_empty() {
        c.admins = vec![me(&state)];
    }
    let link = format!("sulcusai://net/{}/{}/{}", n.id, c.admins.join(","), code);
    if let Some(to) = to {
        if !send(&to, &Wire::Net { msg: NetWire::Invite { card: c, code: code.clone() } }) {
            return Err("They aren't connected right now; send them the link instead.".into());
        }
    }
    Ok(link)
}

/// Reads an invite link: network id, admin devices, code.
pub fn parse_link(link: &str) -> Option<(String, Vec<String>, String)> {
    let rest = link.trim().strip_prefix("sulcusai://net/")?;
    let mut it = rest.split('/');
    let (id, admins, code) = (it.next()?, it.next()?, it.next()?);
    let ok = |s: &str| !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let admins: Vec<String> = admins.split(',').filter(|a| ok(a)).map(String::from).collect();
    (ok(id) && ok(code) && !admins.is_empty()).then(|| (id.into(), admins, code.into()))
}

/// Asks to join: by invite link, by an invite received, or by a network's
/// name (or id) in this PC's groups with its password.
#[tauri::command]
pub fn social_net_join(state: AppStateRef, by: String, password: Option<String>) -> Result<String, String> {
    lan_on(&state)?;
    let peer = my_peer_id(&state)?;
    let (net, admins, invite) = if let Some((id, admins, code)) = parse_link(&by) {
        (id, admins, Some(code))
    } else {
        let invites: Vec<(Card, String)> = db::get(&state.db.lock().unwrap(), "social_net_invites").unwrap_or_default();
        if let Some((c, code)) = invites.into_iter().find(|(c, _)| c.id == by) {
            (c.id, c.admins, Some(code))
        } else {
            let want = by.trim().to_lowercase();
            let found: Vec<Card> = cards(&state).into_values().flatten().filter(|c| c.id == by || c.name.to_lowercase() == want).collect();
            match found.len() {
                0 => return Err("No network by that name in your groups. Check the name, or ask for an invite.".into()),
                1 => {
                    let c = found.into_iter().next().unwrap();
                    (c.id, c.admins, None)
                }
                _ => return Err("More than one network has that name; pick it from the list.".into()),
            }
        }
    };
    let w = Wire::Net { msg: NetWire::Ask { net, peer, name: my_name(&state), password, invite } };
    let sent = admins.iter().filter(|a| send(a, &w)).count();
    if sent == 0 {
        return Err("None of the network's admins are connected right now. Try again when one is online.".into());
    }
    Ok("Asked to join. You'll see the answer here.".into())
}

/// Turns the adapter on for a network (one at a time), like Radmin's power button.
#[tauri::command]
pub async fn social_net_connect(state: AppStateRef<'_>, id: String, on: bool) -> Result<NetsView, String> {
    if on {
        crate::p2p::sharing::vlan_connect(state.clone(), id).await?;
    } else {
        crate::p2p::vlan_adapter::stop();
    }
    if let Some(app) = state.app.get() {
        crate::lan_tray::refresh(app);
    }
    view(&state)
}
