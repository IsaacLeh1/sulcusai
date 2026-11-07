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
    /// A line in the network's own chat.
    Chat { net: String, text: String },
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
        NetWire::Chat { net, text } => {
            // Only from a member's own device, into a network this PC is in.
            let nets = networks(state).unwrap_or_default();
            let Some(n) = nets.iter().find(|n| n.id == net) else { return Ok(()) };
            let Some((peer, _)) = n.devices.iter().find(|(_, d)| d.as_str() == from) else { return Ok(()) };
            if !n.members.contains_key(peer) {
                return Ok(());
            }
            let name = n.names.get(peer).cloned().unwrap_or_else(|| peer.chars().take(8).collect());
            push_chat(state, &net, ChatLine { from: peer.clone(), name, text: text.chars().take(2000).collect(), at: now() })?;
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
    /// Through a relay rather than direct, when connected.
    pub relayed: Option<bool>,
    pub why: Option<String>,
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

pub fn rows(
    n: &Network,
    me: &str,
    online: &dyn Fn(&str) -> (bool, Option<u64>),
    name_fallback: &dyn Fn(&str, Option<&str>) -> Option<String>,
) -> Vec<MemberRow> {
    let mut out: Vec<MemberRow> = n
        .members
        .iter()
        .map(|(p, a)| {
            let (on, ping) = if p == me { (true, Some(0)) } else { online(p) };
            let raw_name = n.names.get(p).cloned().unwrap_or_default();
            let name = if !raw_name.trim().is_empty() {
                raw_name
            } else if let Some(fb) = name_fallback(p, n.devices.get(p).map(|s| s.as_str())) {
                fb
            } else {
                p.chars().take(8).collect()
            };
            MemberRow {
                peer: p.clone(),
                name,
                address: a.to_string(),
                admin: n.admins.contains(p),
                me: p == me,
                online: on,
                ping,
                device: n.devices.get(p).cloned(),
                relayed: None,
                why: None,
            }
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
    crate::p2p::vlan_tools::watch_paths();
    if let Some(n) = up.as_deref().and_then(|id| nets.iter().find(|n| n.id == id)) {
        crate::p2p::vlan_tools::set_members(n.members.iter().filter(|(p, _)| **p != me_peer).map(|(_, a)| *a));
    }
    // The node knows the peer, the watcher knows how it's reached.
    let device_path = |row: &mut MemberRow| {
        if row.online && !row.me {
            if let Some(i) = crate::p2p::vlan_tools::path_of(&row.peer) {
                row.relayed = Some(i.relayed);
                row.why = Some(i.why);
            }
        }
    };
    // my_name and cards take the database lock themselves, so they run before this one is held.
    let my_display_name = my_name(state);
    let open: Vec<Card> = cards(state).into_values().flatten().filter(|c| !nets.iter().any(|n| n.id == c.id)).collect();
    let conn = state.db.lock().unwrap();
    let profiles: BTreeMap<String, super::profile::Profile> = db::get(&conn, "social_profiles").unwrap_or_default();
    let fallback_name = |p: &str, dev: Option<&str>| -> Option<String> {
        if p == me_peer {
            return Some(my_display_name.clone());
        }
        if let Some(dev) = dev {
            if let Some(pr) = profiles.get(dev) {
                if !pr.name.trim().is_empty() {
                    return Some(pr.name.clone());
                }
            }
            for g in &book(&conn).groups {
                if let Some(m) = g.group.members.get(dev) {
                    if !m.name.trim().is_empty() {
                        return Some(m.name.clone());
                    }
                }
            }
        }
        None
    };
    let name_of = |n: &Network, p: &String| {
        let raw = n.names.get(p).cloned().unwrap_or_default();
        let nm = if !raw.trim().is_empty() {
            raw
        } else if let Some(fb) = fallback_name(p, n.devices.get(p).map(|s| s.as_str())) {
            fb
        } else {
            p.chars().take(8).collect()
        };
        (p.clone(), nm)
    };
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
            members: {
                let mut r = rows(n, &me_peer, &online, &fallback_name);
                r.iter_mut().for_each(&device_path);
                r
            },
            waiting: n.waiting.iter().map(|p| name_of(n, p)).collect(),
            banned: n.banned.iter().map(|p| name_of(n, p)).collect(),
        })
        .collect();
    let invites: Vec<(Card, String)> = db::get::<Vec<(Card, String)>>(&conn, "social_net_invites").unwrap_or_default().into_iter().filter(|(c, _)| !nets.iter().any(|n| n.id == c.id)).collect();
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
    if gone {
        // Rules made on join go on leave, whether or not the switch is still on.
        let net = id.clone();
        std::thread::spawn(move || {
            let _ = crate::p2p::vlan_tools::remove_rules(&net);
        });
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
        crate::p2p::sharing::vlan_connect(state.clone(), id.clone()).await?;
        if let Some(app) = state.app.get() {
            crate::p2p::vlan_tools::serve(app.clone());
        }
        let firewall: bool = db::get(&state.db.lock().unwrap(), FIREWALL).unwrap_or(false);
        if firewall {
            let app = state.app.get().cloned();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = crate::p2p::vlan_tools::add_rules(&id) {
                    if let Some(app) = app {
                        let _ = app.emit("sulcus:error", format!("Firewall rule not made: {e}"));
                    }
                }
            });
        }
    } else {
        crate::p2p::vlan_adapter::stop();
    }
    if let Some(app) = state.app.get() {
        crate::lan_tray::refresh(app);
    }
    view(&state)
}

// ---------- per network: chat, member tools, games, sharing ----------

const FIREWALL: &str = "social_net_firewall";
const WAKE: &str = "social_net_wake";
const CHAT: &str = "social_net_chat";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub from: String,
    pub name: String,
    pub text: String,
    pub at: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn push_chat(state: &AppState, net: &str, line: ChatLine) -> Result<(), String> {
    {
        let conn = state.db.lock().unwrap();
        let mut all: BTreeMap<String, Vec<ChatLine>> = db::get(&conn, CHAT).unwrap_or_default();
        let list = all.entry(net.to_string()).or_default();
        list.push(line);
        let extra = list.len().saturating_sub(500);
        list.drain(..extra);
        db::set(&conn, CHAT, &all)?;
    }
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:net_chat", json!({ "net": net }));
    }
    Ok(())
}

fn chat_of(state: &AppState, id: &str) -> Vec<ChatLine> {
    let all: BTreeMap<String, Vec<ChatLine>> = db::get(&state.db.lock().unwrap(), CHAT).unwrap_or_default();
    all.get(id).cloned().unwrap_or_default()
}

#[tauri::command]
pub fn social_net_chat(state: AppStateRef, id: String) -> Vec<ChatLine> {
    chat_of(&state, &id)
}

#[tauri::command]
pub fn social_net_say(state: AppStateRef, id: String, text: String) -> Result<Vec<ChatLine>, String> {
    let text: String = text.trim().chars().take(2000).collect();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let me_peer = my_peer_id(&state)?;
    let nets = networks(&state)?;
    let n = nets.iter().find(|n| n.id == id).ok_or("Unknown network.")?;
    let w = Wire::Net { msg: NetWire::Chat { net: id.clone(), text: text.clone() } };
    for (p, d) in &n.devices {
        if *p != me_peer && n.members.contains_key(p) {
            send(d, &w);
        }
    }
    let name = my_name(&state);
    push_chat(&state, &id, ChatLine { from: me_peer, name, text, at: now() })?;
    Ok(chat_of(&state, &id))
}

fn address_of(state: &AppState, id: &str, peer: &str) -> Result<std::net::Ipv4Addr, String> {
    if crate::p2p::vlan_adapter::running().as_deref() != Some(id) {
        return Err("Turn this network on first.".into());
    }
    let nets = networks(state)?;
    let n = nets.iter().find(|n| n.id == id).ok_or("Unknown network.")?;
    n.members.get(peer).copied().ok_or_else(|| "They aren't in this network.".to_string())
}

#[tauri::command]
pub async fn social_net_speed(state: AppStateRef<'_>, id: String, peer: String) -> Result<crate::p2p::vlan_tools::Speed, String> {
    let to = address_of(&state, &id, &peer)?;
    tokio::task::spawn_blocking(move || crate::p2p::vlan_tools::speed_test(to)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn social_net_send_file(state: AppStateRef<'_>, id: String, peer: String, path: String) -> Result<(), String> {
    let to = address_of(&state, &id, &peer)?;
    tokio::task::spawn_blocking(move || crate::p2p::vlan_tools::send_file(to, std::path::Path::new(&path))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn social_net_remote(state: AppStateRef, id: String, peer: String) -> Result<(), String> {
    crate::p2p::vlan_tools::remote_desktop(address_of(&state, &id, &peer)?)
}

#[tauri::command]
pub fn social_net_received_folder() -> Result<(), String> {
    let dir = crate::p2p::vlan_tools::received_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(&dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct NetExtras {
    pub firewall: bool,
    /// Per network, whether its rule is in Windows now.
    pub rules: BTreeMap<String, bool>,
    pub games: Vec<crate::p2p::vlan_tools::Game>,
    pub shared: Vec<crate::p2p::vlan_tools::Shared>,
    pub sharing_open: bool,
    /// Members of the network that is on: who can see the shares.
    pub can_see: Vec<String>,
    pub my_macs: Vec<String>,
    /// Peer to the card address saved for waking it.
    pub wake: BTreeMap<String, String>,
}

#[tauri::command]
pub async fn social_net_extras(state: AppStateRef<'_>) -> Result<NetExtras, String> {
    let (firewall, wake): (bool, BTreeMap<String, String>) = {
        let conn = state.db.lock().unwrap();
        (db::get(&conn, FIREWALL).unwrap_or(false), db::get(&conn, WAKE).unwrap_or_default())
    };
    let nets = networks(&state).unwrap_or_default();
    let me_peer = my_peer_id(&state).unwrap_or_default();
    let up = crate::p2p::vlan_adapter::running();
    let can_see: Vec<String> = up
        .as_deref()
        .and_then(|id| nets.iter().find(|n| n.id == id))
        .map(|n| n.members.keys().filter(|p| **p != me_peer).map(|p| n.names.get(p).cloned().unwrap_or_else(|| p.chars().take(8).collect())).collect())
        .unwrap_or_default();
    let ids: Vec<String> = nets.iter().map(|n| n.id.clone()).collect();
    let (rules, games, shared, sharing_open, my_macs) = tokio::task::spawn_blocking(move || {
        use crate::p2p::vlan_tools as t;
        let rules: BTreeMap<String, bool> = ids.iter().map(|i| (i.clone(), t::has_rule(i))).collect();
        (rules, t::games(), t::shared(), t::sharing_open(), t::own_macs())
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(NetExtras { firewall, rules, games, shared, sharing_open, can_see, my_macs, wake })
}

#[tauri::command]
pub fn social_net_set_firewall(state: AppStateRef, on: bool) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), FIREWALL, &on)?;
    if on {
        if let Some(id) = crate::p2p::vlan_adapter::running() {
            std::thread::spawn(move || {
                let _ = crate::p2p::vlan_tools::add_rules(&id);
            });
        }
    }
    Ok(())
}

/// Saves (or with an empty `mac`, forgets) a member's card address for waking it.
#[tauri::command]
pub fn social_net_set_wake(state: AppStateRef, peer: String, mac: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut all: BTreeMap<String, String> = db::get(&conn, WAKE).unwrap_or_default();
    if mac.trim().is_empty() {
        all.remove(&peer);
    } else {
        crate::p2p::vlan_tools::parse_mac(mac.trim()).ok_or("That isn't a card address (like 1A-2B-3C-4D-5E-6F).")?;
        all.insert(peer, mac.trim().to_uppercase());
    }
    db::set(&conn, WAKE, &all)
}

/// Waking is allowed only for a member who gave their card address.
#[tauri::command]
pub fn social_net_wake(state: AppStateRef, peer: String) -> Result<(), String> {
    let all: BTreeMap<String, String> = db::get(&state.db.lock().unwrap(), WAKE).unwrap_or_default();
    let mac = all.get(&peer).and_then(|m| crate::p2p::vlan_tools::parse_mac(m)).ok_or("Ask them for their card address (shown under This PC in their Networks tab) and save it first.")?;
    crate::p2p::vlan_tools::wake(mac)
}
