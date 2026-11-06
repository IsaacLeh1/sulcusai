// SPDX-License-Identifier: AGPL-3.0-only
//! The virtual LAN: a private network run by the people in it. Each member
//! gets a fixed address in 10.77.0.0/16; packets go only between members,
//! broadcasts (how LAN games find each other) reach every member, and a
//! member can only send from its own address.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const NET: [u8; 2] = [10, 77];
pub const BROADCAST: Ipv4Addr = Ipv4Addr::new(10, 77, 255, 255);
pub const MAX_MEMBERS: usize = 250;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    /// PeerIds that may add and remove members; the creator first.
    pub admins: BTreeSet<String>,
    /// PeerId to address.
    pub members: BTreeMap<String, Ipv4Addr>,
    /// Join requests waiting for an admin.
    pub waiting: BTreeSet<String>,
    /// Goes up with every change, so members keep only newer member lists.
    #[serde(default)]
    pub version: u64,
    /// The Social group it belongs to, if any.
    #[serde(default)]
    pub group: Option<String>,
    /// Salted hash of the join password; empty when there is none.
    #[serde(default)]
    pub password: String,
    /// Every join waits for an admin, even with the password.
    #[serde(default)]
    pub approval: bool,
    #[serde(default)]
    pub banned: BTreeSet<String>,
    /// Most members allowed (0: up to MAX_MEMBERS).
    #[serde(default)]
    pub limit: usize,
    /// PeerId to the name shown for it.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// PeerId to that member's Social device, where join requests and chats go.
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
    /// Hashes of invite codes that let one in without the password.
    #[serde(default)]
    pub invites: BTreeSet<String>,
}

/// What a join request comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Join {
    Admitted(Ipv4Addr),
    Waiting,
}

fn salted(id: &str, secret: &str) -> String {
    hex::encode(Sha256::digest(format!("sulcus-vlan-join\n{id}\n{secret}").as_bytes()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotAdmin,
    Full,
    NotMember,
    LastAdmin,
    Banned,
    WrongPassword,
}

impl Refused {
    pub fn message(&self) -> &'static str {
        match self {
            Refused::NotAdmin => "Only the network's admins can do that.",
            Refused::Full => "The network is full.",
            Refused::NotMember => "They aren't in the network.",
            Refused::LastAdmin => "Make someone else an admin first.",
            Refused::Banned => "You can't join this network.",
            Refused::WrongPassword => "That isn't the network's password.",
        }
    }
}

fn usable(a: Ipv4Addr) -> bool {
    let o = a.octets();
    !(o[2] == 0 && o[3] == 0) && a != BROADCAST && o[3] != 0 && o[3] != 255
}

/// A stable address for a peer, moved along if another member holds it.
pub fn address_for(peer: &str, taken: &BTreeSet<Ipv4Addr>) -> Ipv4Addr {
    let h = Sha256::digest(peer.as_bytes());
    let mut n = u16::from_be_bytes([h[0], h[1]]);
    loop {
        let a = Ipv4Addr::new(NET[0], NET[1], (n >> 8) as u8, n as u8);
        if usable(a) && !taken.contains(&a) {
            return a;
        }
        n = n.wrapping_add(1);
    }
}

impl Network {
    pub fn create(id: &str, name: &str, owner: &str) -> Network {
        let mut n = Network { id: id.into(), name: name.into(), ..Default::default() };
        n.admins.insert(owner.into());
        let a = address_for(owner, &BTreeSet::new());
        n.members.insert(owner.into(), a);
        n
    }

    pub fn ask(&mut self, peer: &str) {
        if !self.members.contains_key(peer) {
            self.waiting.insert(peer.into());
        }
    }

    pub fn admit(&mut self, admin: &str, peer: &str) -> Result<Ipv4Addr, Refused> {
        if !self.admins.contains(admin) {
            return Err(Refused::NotAdmin);
        }
        if let Some(a) = self.members.get(peer) {
            return Ok(*a);
        }
        if self.banned.contains(peer) {
            return Err(Refused::Banned);
        }
        if self.members.len() >= self.max() {
            return Err(Refused::Full);
        }
        let taken = self.members.values().copied().collect();
        let a = address_for(peer, &taken);
        self.waiting.remove(peer);
        self.members.insert(peer.into(), a);
        self.version += 1;
        Ok(a)
    }

    fn max(&self) -> usize {
        if self.limit == 0 { MAX_MEMBERS } else { self.limit.min(MAX_MEMBERS) }
    }

    fn admin(&self, by: &str) -> Result<(), Refused> {
        if self.admins.contains(by) { Ok(()) } else { Err(Refused::NotAdmin) }
    }

    pub fn rename(&mut self, by: &str, name: &str) -> Result<(), Refused> {
        self.admin(by)?;
        self.name = name.trim().chars().take(60).collect();
        self.version += 1;
        Ok(())
    }

    /// Password (Some("") removes it, None keeps it), approval and member limit.
    pub fn set_rules(&mut self, by: &str, password: Option<&str>, approval: bool, limit: usize) -> Result<(), Refused> {
        self.admin(by)?;
        if let Some(p) = password {
            self.password = if p.is_empty() { String::new() } else { salted(&self.id, p) };
        }
        self.approval = approval;
        self.limit = limit.min(MAX_MEMBERS);
        self.version += 1;
        Ok(())
    }

    pub fn has_password(&self) -> bool {
        !self.password.is_empty()
    }

    /// An invite code an admin hands out; it lets the holder in without the password.
    pub fn add_invite(&mut self, by: &str, code: &str) -> Result<(), Refused> {
        self.admin(by)?;
        self.invites.insert(salted(&self.id, code));
        self.version += 1;
        Ok(())
    }

    /// A join request handled on an admin's PC: in at once with a good
    /// invite, or with the password when no approval is needed; otherwise it
    /// waits for an admin.
    pub fn join(&mut self, admin: &str, peer: &str, password: Option<&str>, invite: Option<&str>) -> Result<Join, Refused> {
        self.admin(admin)?;
        if self.banned.contains(peer) {
            return Err(Refused::Banned);
        }
        if let Some(a) = self.members.get(peer) {
            return Ok(Join::Admitted(*a));
        }
        if invite.is_some_and(|c| self.invites.contains(&salted(&self.id, c))) {
            return self.admit(admin, peer).map(Join::Admitted);
        }
        if self.has_password() && password.map(|p| salted(&self.id, p)).as_deref() != Some(self.password.as_str()) {
            return Err(Refused::WrongPassword);
        }
        if self.approval {
            self.ask(peer);
            return Ok(Join::Waiting);
        }
        self.admit(admin, peer).map(Join::Admitted)
    }

    /// Removes and keeps out.
    pub fn ban(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        self.admin(by)?;
        if self.members.contains_key(peer) {
            self.remove(by, peer)?;
        }
        self.waiting.remove(peer);
        self.banned.insert(peer.into());
        self.version += 1;
        Ok(())
    }

    pub fn unban(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        self.admin(by)?;
        self.banned.remove(peer);
        self.version += 1;
        Ok(())
    }

    pub fn deny(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        self.admin(by)?;
        self.waiting.remove(peer);
        Ok(())
    }

    pub fn demote(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        self.admin(by)?;
        if self.admins.contains(peer) && self.admins.len() == 1 {
            return Err(Refused::LastAdmin);
        }
        self.admins.remove(peer);
        self.version += 1;
        Ok(())
    }

    /// Admins remove anyone; anyone may leave on their own.
    pub fn remove(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        if by != peer && !self.admins.contains(by) {
            return Err(Refused::NotAdmin);
        }
        if self.admins.contains(peer) && self.admins.len() == 1 && self.members.len() > 1 {
            return Err(Refused::LastAdmin);
        }
        self.members.remove(peer).ok_or(Refused::NotMember)?;
        self.admins.remove(peer);
        self.version += 1;
        Ok(())
    }

    pub fn make_admin(&mut self, by: &str, peer: &str) -> Result<(), Refused> {
        if !self.admins.contains(by) {
            return Err(Refused::NotAdmin);
        }
        if !self.members.contains_key(peer) {
            return Err(Refused::NotMember);
        }
        self.admins.insert(peer.into());
        self.version += 1;
        Ok(())
    }

    fn by_addr(&self, a: Ipv4Addr) -> Option<&String> {
        self.members.iter().find(|(_, x)| **x == a).map(|(p, _)| p)
    }

    /// Where an IPv4 packet from `sender` goes: the member peers to send it
    /// to, or why it was dropped.
    pub fn route(&self, sender: &str, packet: &[u8]) -> Result<Vec<String>, Drop> {
        let mine = *self.members.get(sender).ok_or(Drop::NotMember)?;
        if packet.len() < 20 || packet[0] >> 4 != 4 {
            return Err(Drop::NotIpv4);
        }
        let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
        let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
        if src != mine {
            return Err(Drop::Spoofed);
        }
        if dst == BROADCAST || dst == Ipv4Addr::BROADCAST || dst.is_multicast() {
            return Ok(self.members.keys().filter(|p| *p != sender).cloned().collect());
        }
        match self.by_addr(dst) {
            Some(p) if p != sender => Ok(vec![p.clone()]),
            _ => Err(Drop::NoSuchMember),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Drop {
    NotMember,
    NotIpv4,
    Spoofed,
    NoSuchMember,
}

/// The kinds of VPN shown together on the networks page, by adapter name.
pub fn vpn_kind(adapter: &str) -> Option<&'static str> {
    let a = adapter.to_ascii_lowercase();
    const KNOWN: [(&str, &str); 10] = [
        ("sulcus", "SulcusAI virtual LAN"),
        ("radmin", "Radmin VPN"),
        ("zerotier", "ZeroTier"),
        ("tailscale", "Tailscale"),
        ("wireguard", "WireGuard"),
        ("wg", "WireGuard"),
        ("openvpn", "OpenVPN"),
        ("tap-windows", "OpenVPN"),
        ("pangp", "GlobalProtect"),
        ("hamachi", "Hamachi"),
    ];
    KNOWN.iter().find(|(k, _)| if *k == "wg" { a.starts_with("wg") } else { a.contains(k) }).map(|(_, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
        let mut p = vec![0u8; 28];
        p[0] = 0x45;
        p[12..16].copy_from_slice(&src.octets());
        p[16..20].copy_from_slice(&dst.octets());
        p
    }

    #[test]
    fn members_get_stable_distinct_addresses() {
        let mut n = Network::create("n1", "Friday games", "alice");
        assert_eq!(n.admit("bob", "carol"), Err(Refused::NotAdmin));
        n.ask("bob");
        let b = n.admit("alice", "bob").unwrap();
        assert!(n.waiting.is_empty());
        assert_eq!(n.admit("alice", "bob").unwrap(), b);
        let c = n.admit("alice", "carol").unwrap();
        assert_ne!(b, c);
        assert_eq!(b, address_for("bob", &BTreeSet::new()));
        assert_eq!(&b.octets()[..2], &NET);
        let taken: BTreeSet<_> = [b].into();
        assert_ne!(address_for("bob", &taken), b);
    }

    #[test]
    fn routing_unicast_broadcast_and_spoofing() {
        let mut n = Network::create("n1", "x", "alice");
        let a = n.members["alice"];
        let b = n.admit("alice", "bob").unwrap();
        n.admit("alice", "carol").unwrap();
        assert_eq!(n.route("alice", &packet(a, b)).unwrap(), vec!["bob".to_string()]);
        let mut all = n.route("alice", &packet(a, BROADCAST)).unwrap();
        all.sort();
        assert_eq!(all, vec!["bob".to_string(), "carol".to_string()]);
        assert_eq!(n.route("alice", &packet(a, Ipv4Addr::BROADCAST)).unwrap().len(), 2);
        assert_eq!(n.route("bob", &packet(a, b)), Err(Drop::Spoofed));
        assert_eq!(n.route("eve", &packet(a, b)), Err(Drop::NotMember));
        assert_eq!(n.route("alice", &packet(a, Ipv4Addr::new(8, 8, 8, 8))), Err(Drop::NoSuchMember));
        assert_eq!(n.route("alice", &[0x60; 40]), Err(Drop::NotIpv4));
    }

    #[test]
    fn leaving_and_admins() {
        let mut n = Network::create("n1", "x", "alice");
        n.admit("alice", "bob").unwrap();
        assert_eq!(n.remove("bob", "alice"), Err(Refused::NotAdmin));
        assert_eq!(n.remove("alice", "alice"), Err(Refused::LastAdmin));
        n.make_admin("alice", "bob").unwrap();
        n.remove("alice", "alice").unwrap();
        assert_eq!(n.admins.len(), 1);
        n.remove("bob", "bob").unwrap();
        assert!(n.members.is_empty());
    }

    #[test]
    fn passwords_approval_bans_and_limits() {
        let mut n = Network::create("n1", "x", "alice");
        assert_eq!(n.join("bob", "eve", None, None), Err(Refused::NotAdmin));
        n.set_rules("alice", Some("hunter2"), false, 3).unwrap();
        assert_eq!(n.join("alice", "bob", Some("nope"), None), Err(Refused::WrongPassword));
        assert!(matches!(n.join("alice", "bob", Some("hunter2"), None), Ok(Join::Admitted(_))));
        n.set_rules("alice", None, true, 3).unwrap();
        assert!(n.has_password(), "None keeps the password");
        assert_eq!(n.join("alice", "carol", Some("hunter2"), None), Ok(Join::Waiting));
        assert!(n.waiting.contains("carol"));
        n.add_invite("alice", "CODE").unwrap();
        assert!(matches!(n.join("alice", "dan", None, Some("CODE")), Ok(Join::Admitted(_))));
        assert_eq!(n.admit("alice", "carol"), Err(Refused::Full));
        n.ban("alice", "bob").unwrap();
        assert!(!n.members.contains_key("bob"));
        assert_eq!(n.join("alice", "bob", Some("hunter2"), Some("CODE")), Err(Refused::Banned));
        n.unban("alice", "bob").unwrap();
        assert_eq!(n.demote("alice", "alice"), Err(Refused::LastAdmin));
        n.rename("alice", "  Friday  ").unwrap();
        assert_eq!(n.name, "Friday");
        // Networks saved before these fields still open.
        let old: Network = serde_json::from_str(r#"{"id":"o","name":"o","admins":[],"members":{},"waiting":[]}"#).unwrap();
        assert_eq!(old.limit, 0);
    }

    #[test]
    fn vpn_names() {
        assert_eq!(vpn_kind("Radmin VPN"), Some("Radmin VPN"));
        assert_eq!(vpn_kind("wg0"), Some("WireGuard"));
        assert_eq!(vpn_kind("PANGP Virtual Ethernet Adapter"), Some("GlobalProtect"));
        assert_eq!(vpn_kind("Ethernet"), None);
    }
}
