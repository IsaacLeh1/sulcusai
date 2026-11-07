// SPDX-License-Identifier: AGPL-3.0-only
//! Groups: invite-only, run by their owners and moderators.
//!
//! Every change is an `Op` from a member, applied in the order the owner's
//! PC numbered it, so every member's copy ends the same. Permissions are
//! checked on apply, not trusted from the sender: a copy that skipped a check
//! still can't make others accept the change.

use std::collections::BTreeMap;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::MemberId;

pub const MAX_MEMBERS: usize = 500;
pub const MAX_CHANNELS: usize = 200;
pub const MAX_NAME: usize = 100;
/// Invites last a week at most, so a leaked one goes stale on its own.
pub const MAX_INVITE_MS: i64 = 7 * 24 * 3600 * 1000;
const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    Moderator,
    Owner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: Role,
    /// Teen settings apply to this member (set from their own PC).
    pub teen: bool,
    /// Muted in the group until then (ms), by a moderator.
    pub muted_until: i64,
    /// An assistant member, run on `host`'s PC.
    pub assistant: Option<MemberId>,
    /// The group's own roles this member has, by role id.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Who made the invite they joined with, so inviters can see who came.
    #[serde(default)]
    pub invited_by: Option<MemberId>,
}

/// A role the group made for itself: a name, a colour, and channel overrides
/// under "role:<id>" in each channel's perms. Moderation stays with `Role`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomRole {
    pub id: String,
    pub name: String,
    /// "#rrggbb", or empty for no colour.
    #[serde(default)]
    pub color: String,
    /// Listed apart in the member list.
    #[serde(default)]
    pub hoist: bool,
}

pub const MAX_ROLES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Voice,
    /// Only moderators post.
    Announcements,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    /// Seconds between one member's messages; 0 is off.
    pub slow_secs: u32,
    /// Hidden from teen members.
    pub adults_only: bool,
    /// What the channel is for, shown in its header.
    #[serde(default)]
    pub topic: String,
    /// Voice: most people in the call at once; 0 is no limit.
    #[serde(default)]
    pub user_limit: u32,
    /// Voice: kilobits a second for each voice; 0 is the default.
    #[serde(default)]
    pub bitrate: u32,
    /// Only moderators and the people or roles allowed below see it.
    #[serde(default)]
    pub private: bool,
    /// Overrides by "role:member", "role:moderator" or a member id.
    #[serde(default)]
    pub perms: BTreeMap<String, Perms>,
}

/// One override: each `Some` allows or denies, `None` leaves it as it was.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Perms {
    #[serde(default)]
    pub view: Option<bool>,
    #[serde(default)]
    pub send: Option<bool>,
    #[serde(default)]
    pub connect: Option<bool>,
    #[serde(default)]
    pub speak: Option<bool>,
}

/// What can be allowed or denied in a channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Perm {
    View,
    Send,
    Connect,
    Speak,
}

impl Perms {
    fn get(&self, p: Perm) -> Option<bool> {
        match p {
            Perm::View => self.view,
            Perm::Send => self.send,
            Perm::Connect => self.connect,
            Perm::Speak => self.speak,
        }
    }
}

/// The settings a moderator can change on a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSettings {
    pub name: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub slow_secs: u32,
    #[serde(default)]
    pub user_limit: u32,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub adults_only: bool,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub perms: BTreeMap<String, Perms>,
}

impl Channel {
    pub fn new(id: String, name: String, kind: ChannelKind, adults_only: bool) -> Self {
        Channel { id, name, kind, slow_secs: 0, adults_only, topic: String::new(), user_limit: 0, bitrate: 0, private: false, perms: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    /// Only the hash is kept, so a copy of the group can't hand out invites.
    pub hash: String,
    pub expires: i64,
    pub uses_left: u32,
    pub by: MemberId,
}

/// The group key at one turn. A new turn starts whenever someone leaves or is
/// removed, so they can't read what comes after.
#[derive(Clone)]
pub struct GroupKey {
    pub epoch: u32,
    key: Zeroizing<[u8; 32]>,
}

impl GroupKey {
    pub fn random(epoch: u32) -> GroupKey {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(key.as_mut());
        GroupKey { epoch, key }
    }

    pub fn from_bytes(epoch: u32, bytes: [u8; 32]) -> GroupKey {
        GroupKey { epoch, key: Zeroizing::new(bytes) }
    }

    pub fn bytes(&self) -> [u8; 32] {
        *self.key
    }

    /// Seals `plain`; `bound` (group, channel, turn) is checked on open so a
    /// message can't be replayed into another channel or group.
    pub fn seal(&self, bound: &str, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let aad = format!("{bound}|{}", self.epoch);
        let ct = Aes256Gcm::new(self.key.as_ref().into())
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: aad.as_bytes() })
            .expect("AES-GCM encryption");
        let mut out = Vec::with_capacity(4 + NONCE_LEN + ct.len());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, bound: &str, blob: &[u8]) -> Result<Vec<u8>, String> {
        if blob.len() < 4 + NONCE_LEN {
            return Err("A group message was cut short.".into());
        }
        let epoch = u32::from_le_bytes(blob[..4].try_into().unwrap());
        if epoch != self.epoch {
            return Err("A group message was sealed with another key.".into());
        }
        let aad = format!("{bound}|{epoch}");
        Aes256Gcm::new(self.key.as_ref().into())
            .decrypt(Nonce::from_slice(&blob[4..4 + NONCE_LEN]), Payload { msg: &blob[4 + NONCE_LEN..], aad: aad.as_bytes() })
            .map_err(|_| "A group message couldn't be opened.".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub members: BTreeMap<MemberId, Member>,
    pub channels: Vec<Channel>,
    pub invites: Vec<Invite>,
    pub banned: Vec<MemberId>,
    /// The turn of the group key; members must hold the key for it.
    pub epoch: u32,
    /// Ops applied so far, so a member can tell what it missed.
    pub seq: u64,
    /// Whether teens may join at all.
    pub teens_allowed: bool,
    /// Roles the group made, highest first.
    #[serde(default)]
    pub roles: Vec<CustomRole>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Rename { name: String },
    AddChannel { id: String, name: String, kind: ChannelKind, adults_only: bool },
    RemoveChannel { id: String },
    EditChannel { id: String, settings: ChannelSettings },
    SlowMode { channel: String, secs: u32 },
    Invite { hash: String, expires: i64, uses: u32 },
    Join { hash: String, name: String, teen: bool },
    Leave,
    SetRole { member: MemberId, role: Role },
    Kick { member: MemberId },
    Ban { member: MemberId },
    Unban { member: MemberId },
    Mute { member: MemberId, until: i64 },
    AddAssistant { id: MemberId, name: String },
    TeensAllowed { allowed: bool },
    /// Adds a role, or changes the one with the same id.
    SaveRole { role: CustomRole },
    RemoveRole { id: String },
    GiveRole { member: MemberId, role: String, on: bool },
}

/// What applying an op means for the group key.
#[derive(Debug, PartialEq)]
pub enum Applied {
    Done,
    /// Someone left: the owner's PC hands out a key for the new turn.
    NewKey,
}

pub fn invite_hash(code: &str) -> String {
    let clean: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    hex(&Sha256::digest(clean.as_bytes()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn name_ok(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
        return Err("Names are 1 to 100 characters.".into());
    }
    Ok(n.to_string())
}

impl Group {
    pub fn new(id: &str, name: &str, owner: &str, owner_name: &str) -> Result<Group, String> {
        let mut members = BTreeMap::new();
        members.insert(owner.to_string(), Member { name: name_ok(owner_name)?, role: Role::Owner, teen: false, muted_until: 0, assistant: None, tags: vec![], invited_by: None });
        Ok(Group {
            id: id.into(),
            name: name_ok(name)?,
            members,
            channels: vec![Channel::new("general".into(), "general".into(), ChannelKind::Text, false)],
            invites: vec![],
            banned: vec![],
            epoch: 0,
            seq: 0,
            teens_allowed: false,
            roles: vec![],
        })
    }

    pub fn role(&self, who: &str) -> Option<Role> {
        self.members.get(who).map(|m| m.role)
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Channels `who` can see: teens don't see adults-only ones.
    pub fn visible(&self, who: &str) -> Vec<&Channel> {
        let teen = self.members.get(who).map(|m| m.teen).unwrap_or(true);
        self.channels.iter().filter(|c| !(teen && c.adults_only) && self.can(who, &c.id, Perm::View)).collect()
    }

    /// Whether `who` may do `p` in `channel`: moderators always may; then
    /// the channel's default, its role override, then their own override.
    pub fn can(&self, who: &str, channel: &str, p: Perm) -> bool {
        let (Some(m), Some(ch)) = (self.members.get(who), self.channel(channel)) else { return false };
        if m.role >= Role::Moderator {
            return true;
        }
        if ch.adults_only && m.teen {
            return false;
        }
        let mut on = !(ch.private && matches!(p, Perm::View | Perm::Connect));
        let role = format!("role:{}", if m.role == Role::Moderator { "moderator" } else { "member" });
        if let Some(v) = ch.perms.get(&role).and_then(|o| o.get(p)) {
            on = v;
        }
        // The group's own roles: any one allowing wins over the rest denying,
        // so giving someone a role only ever adds to what they can do there.
        let custom: Vec<bool> = m.tags.iter().filter_map(|t| ch.perms.get(&format!("role:{t}")).and_then(|o| o.get(p))).collect();
        if !custom.is_empty() {
            on = custom.contains(&true);
        }
        if let Some(v) = ch.perms.get(who).and_then(|o| o.get(p)) {
            on = v;
        }
        // Can't send or speak where they can't see or join.
        match p {
            Perm::Send => on && self.can(who, channel, Perm::View),
            Perm::Speak => on && self.can(who, channel, Perm::Connect),
            Perm::Connect => on && self.can(who, channel, Perm::View),
            Perm::View => on,
        }
    }

    fn at_least(&self, who: &str, role: Role) -> Result<(), String> {
        match self.role(who) {
            Some(r) if r >= role => Ok(()),
            Some(_) => Err("You don't have permission for that in this group.".into()),
            None => Err("You aren't in this group.".into()),
        }
    }

    /// Whether `actor` may act on `target`: only on members below them.
    fn above(&self, actor: &str, target: &str) -> Result<(), String> {
        let a = self.role(actor).ok_or("You aren't in this group.")?;
        let t = self.role(target).unwrap_or(Role::Member);
        if a < Role::Moderator || a <= t {
            return Err("You can only act on members below you.".into());
        }
        Ok(())
    }

    pub fn apply(&mut self, actor: &str, op: Op, now: i64) -> Result<Applied, String> {
        let mut applied = Applied::Done;
        match op {
            Op::Join { hash, name, teen } => {
                if self.members.contains_key(actor) {
                    return Err("Already in this group.".into());
                }
                if self.banned.iter().any(|b| b == actor) {
                    return Err("This person is banned from the group.".into());
                }
                if teen && !self.teens_allowed {
                    return Err("This group doesn't take teen members.".into());
                }
                if self.members.len() >= MAX_MEMBERS {
                    return Err("This group is full.".into());
                }
                let inv = self.invites.iter_mut().find(|i| i.hash == hash && i.expires > now && i.uses_left > 0).ok_or("That invite isn't valid any more.")?;
                inv.uses_left -= 1;
                let by = inv.by.clone();
                self.invites.retain(|i| i.uses_left > 0 && i.expires > now);
                self.members.insert(actor.into(), Member { name: name_ok(&name)?, role: Role::Member, teen, muted_until: 0, assistant: None, tags: vec![], invited_by: Some(by) });
            }
            Op::Leave => {
                if self.role(actor) == Some(Role::Owner) && self.members.values().filter(|m| m.role == Role::Owner).count() == 1 && self.members.len() > 1 {
                    return Err("Make someone else an owner before leaving.".into());
                }
                self.members.remove(actor).ok_or("You aren't in this group.")?;
                self.drop_assistants_of(actor);
                applied = Applied::NewKey;
            }
            Op::Rename { name } => {
                self.at_least(actor, Role::Moderator)?;
                self.name = name_ok(&name)?;
            }
            Op::AddChannel { id, name, kind, adults_only } => {
                self.at_least(actor, Role::Moderator)?;
                if self.channels.len() >= MAX_CHANNELS || self.channel(&id).is_some() {
                    return Err("That channel can't be added.".into());
                }
                self.channels.push(Channel::new(id, name_ok(&name)?, kind, adults_only));
            }
            Op::RemoveChannel { id } => {
                self.at_least(actor, Role::Moderator)?;
                self.channels.retain(|c| c.id != id);
            }
            Op::EditChannel { id, settings: c } => {
                self.at_least(actor, Role::Moderator)?;
                let name = name_ok(&c.name)?;
                if c.topic.chars().count() > 1024 || c.perms.len() > 100 || c.perms.keys().any(|k| k.len() > 100) {
                    return Err("Those channel settings are too long.".into());
                }
                let ch = self.channels.iter_mut().find(|x| x.id == id).ok_or("No such channel.")?;
                ch.name = name;
                ch.topic = c.topic;
                ch.slow_secs = c.slow_secs.min(6 * 3600);
                ch.user_limit = c.user_limit.min(99);
                ch.bitrate = if c.bitrate == 0 { 0 } else { c.bitrate.clamp(8, 384) };
                ch.adults_only = c.adults_only;
                ch.private = c.private;
                ch.perms = c.perms;
            }
            Op::SlowMode { channel, secs } => {
                self.at_least(actor, Role::Moderator)?;
                let c = self.channels.iter_mut().find(|c| c.id == channel).ok_or("No such channel.")?;
                c.slow_secs = secs.min(6 * 3600);
            }
            Op::Invite { hash, expires, uses } => {
                self.at_least(actor, Role::Moderator)?;
                if hash.len() != 64 {
                    return Err("That invite isn't valid.".into());
                }
                self.invites.push(Invite { hash, expires: expires.min(now + MAX_INVITE_MS), uses_left: uses.clamp(1, 50), by: actor.into() });
            }
            Op::SetRole { member, role } => {
                self.at_least(actor, Role::Owner)?;
                if !self.members.contains_key(&member) {
                    return Err("They aren't in this group.".into());
                }
                if member == actor && role != Role::Owner && self.members.values().filter(|m| m.role == Role::Owner).count() == 1 {
                    return Err("A group needs an owner.".into());
                }
                self.members.get_mut(&member).unwrap().role = role;
            }
            Op::Kick { member } => {
                self.above(actor, &member)?;
                self.members.remove(&member);
                self.drop_assistants_of(&member);
                applied = Applied::NewKey;
            }
            Op::Ban { member } => {
                self.above(actor, &member)?;
                self.members.remove(&member);
                self.drop_assistants_of(&member);
                if !self.banned.contains(&member) {
                    self.banned.push(member);
                }
                applied = Applied::NewKey;
            }
            Op::Unban { member } => {
                self.at_least(actor, Role::Moderator)?;
                self.banned.retain(|b| *b != member);
            }
            Op::Mute { member, until } => {
                self.above(actor, &member)?;
                self.members.get_mut(&member).ok_or("They aren't in this group.")?.muted_until = until;
            }
            Op::AddAssistant { id, name } => {
                self.at_least(actor, Role::Moderator)?;
                if self.members.contains_key(&id) {
                    return Err("That name is taken in this group.".into());
                }
                self.members.insert(id, Member { name: name_ok(&name)?, role: Role::Member, teen: false, muted_until: 0, assistant: Some(actor.into()), tags: vec![], invited_by: None });
            }
            Op::TeensAllowed { allowed } => {
                self.at_least(actor, Role::Owner)?;
                self.teens_allowed = allowed;
            }
            Op::SaveRole { role } => {
                self.at_least(actor, Role::Moderator)?;
                let colour_ok = role.color.is_empty() || (role.color.len() == 7 && role.color.starts_with('#') && role.color[1..].chars().all(|x| x.is_ascii_hexdigit()));
                // "member", "moderator" and "owner" already name the built-in roles in channel overrides.
                if role.id.is_empty() || role.id.len() > 40 || !role.id.chars().all(|c| c.is_ascii_alphanumeric()) || ["member", "moderator", "owner"].contains(&role.id.as_str()) || !colour_ok {
                    return Err("That role isn't valid.".into());
                }
                let role = CustomRole { name: name_ok(&role.name)?, ..role };
                match self.roles.iter_mut().find(|r| r.id == role.id) {
                    Some(r) => *r = role,
                    None if self.roles.len() < MAX_ROLES => self.roles.push(role),
                    None => return Err("A group has 50 roles at most.".into()),
                }
            }
            Op::RemoveRole { id } => {
                self.at_least(actor, Role::Moderator)?;
                self.roles.retain(|r| r.id != id);
                for m in self.members.values_mut() {
                    m.tags.retain(|t| *t != id);
                }
                for ch in &mut self.channels {
                    ch.perms.remove(&format!("role:{id}"));
                }
            }
            Op::GiveRole { member, role, on } => {
                self.at_least(actor, Role::Moderator)?;
                if !self.roles.iter().any(|r| r.id == role) {
                    return Err("No such role.".into());
                }
                let m = self.members.get_mut(&member).ok_or("They aren't in this group.")?;
                m.tags.retain(|t| *t != role);
                if on {
                    m.tags.push(role);
                }
            }
        }
        self.seq += 1;
        if applied == Applied::NewKey {
            self.epoch += 1;
        }
        Ok(applied)
    }

    /// An assistant goes with the member whose PC runs it.
    fn drop_assistants_of(&mut self, host: &str) {
        self.members.retain(|_, m| m.assistant.as_deref() != Some(host));
    }

    /// Who gets the key for a new turn: every member's own PC (assistants
    /// read through their host).
    pub fn key_holders(&self) -> Vec<MemberId> {
        self.members.iter().filter(|(_, m)| m.assistant.is_none()).map(|(id, _)| id.clone()).collect()
    }
}

/// A fresh invite code to read aloud or paste (Crockford-style, 16 chars).
pub fn new_invite_code() -> String {
    const ALPHA: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut b = [0u8; 16];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| ALPHA[(*x as usize) % ALPHA.len()] as char).collect()
}
