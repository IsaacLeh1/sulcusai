// SPDX-License-Identifier: AGPL-3.0-only
//! Group letters with MLS (RFC 9420, through OpenMLS): a group's members
//! share keys that change with every add and remove, so a hub or relay
//! carrying the messages can't read, forge or replay them, and someone
//! removed can't read what comes after. Each member's credential is its
//! node id (PeerId), so a letter's sender is the PC that signed it.
//!
//! Groups live in memory here; the app keeps them sealed on disk.

use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

/// X25519, ChaCha20-Poly1305, SHA-256 and Ed25519, the same family as the
/// rest of the network code.
pub const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;

/// Epochs whose keys stay to open letters that were sent just before a
/// change and arrive after it.
const PAST_EPOCHS: usize = 5;

/// Letters from one sender may arrive this far out of order.
fn ratchet() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(100, 1000)
}

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

/// One PC's MLS identity: its node id and a signing key for this scheme.
pub struct Member {
    pub peer: String,
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    cred: CredentialWithKey,
}

impl Member {
    pub fn new(peer: &str) -> Result<Self, String> {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).map_err(err)?;
        signer.store(provider.storage()).map_err(err)?;
        let cred = CredentialWithKey { credential: BasicCredential::new(peer.as_bytes().to_vec()).into(), signature_key: signer.public().to_vec().into() };
        Ok(Member { peer: peer.to_string(), provider, signer, cred })
    }

    /// A key package to hand to whoever adds this PC to a group.
    pub fn key_package(&self) -> Result<Vec<u8>, String> {
        let kp = KeyPackage::builder().build(SUITE, &self.provider, &self.signer, self.cred.clone()).map_err(err)?;
        kp.key_package().tls_serialize_detached().map_err(err)
    }
}

pub struct Group {
    me: Member,
    group: MlsGroup,
}

/// What a received message turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    /// A letter, with the node id that signed it.
    Letter { from: String, body: Vec<u8> },
    /// A change of members, now applied.
    Changed,
}

impl Group {
    /// A new group with only this PC in it.
    pub fn create(me: Member) -> Result<Self, String> {
        let config = MlsGroupCreateConfig::builder().ciphersuite(SUITE).use_ratchet_tree_extension(true).max_past_epochs(PAST_EPOCHS).sender_ratchet_configuration(ratchet()).build();
        let group = MlsGroup::new(&me.provider, &me.signer, &config, me.cred.clone()).map_err(err)?;
        Ok(Group { me, group })
    }

    /// Joins from the welcome someone sent after adding this PC.
    pub fn join(me: Member, welcome: &[u8]) -> Result<Self, String> {
        let msg = MlsMessageIn::tls_deserialize(&mut &welcome[..]).map_err(err)?;
        let MlsMessageBodyIn::Welcome(w) = msg.extract() else { return Err("not a welcome".into()) };
        let staged = StagedWelcome::new_from_welcome(&me.provider, &MlsGroupJoinConfig::builder().max_past_epochs(PAST_EPOCHS).sender_ratchet_configuration(ratchet()).build(), w, None).map_err(err)?;
        let group = staged.into_group(&me.provider).map_err(err)?;
        Ok(Group { me, group })
    }

    /// Adds a PC by its key package: the commit for the members already in
    /// and the welcome for the newcomer.
    pub fn add(&mut self, key_package: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let kp = KeyPackageIn::tls_deserialize(&mut &key_package[..]).map_err(err)?.validate(self.me.provider.crypto(), ProtocolVersion::Mls10).map_err(err)?;
        let (commit, welcome, _) = self.group.add_members(&self.me.provider, &self.me.signer, &[kp]).map_err(err)?;
        self.group.merge_pending_commit(&self.me.provider).map_err(err)?;
        Ok((commit.tls_serialize_detached().map_err(err)?, welcome.tls_serialize_detached().map_err(err)?))
    }

    /// Removes a PC by node id; the commit re-keys the group without it.
    pub fn remove(&mut self, peer: &str) -> Result<Vec<u8>, String> {
        let idx = self.group.members().find(|m| m.credential.serialized_content() == peer.as_bytes()).map(|m| m.index).ok_or("not a member")?;
        let (commit, _, _) = self.group.remove_members(&self.me.provider, &self.me.signer, &[idx]).map_err(err)?;
        self.group.merge_pending_commit(&self.me.provider).map_err(err)?;
        commit.tls_serialize_detached().map_err(err)
    }

    pub fn members(&self) -> Vec<String> {
        self.group.members().map(|m| String::from_utf8_lossy(m.credential.serialized_content()).into_owned()).collect()
    }

    /// Seals a letter for everyone in the group now.
    pub fn send(&mut self, body: &[u8]) -> Result<Vec<u8>, String> {
        self.group.create_message(&self.me.provider, &self.me.signer, body).map_err(err)?.tls_serialize_detached().map_err(err)
    }

    /// Opens a letter or applies a change. Replays, forgeries and letters
    /// from an older epoch are refused by MLS.
    pub fn receive(&mut self, raw: &[u8]) -> Result<Received, String> {
        let msg = MlsMessageIn::tls_deserialize(&mut &raw[..]).map_err(err)?;
        let pm = msg.try_into_protocol_message().map_err(err)?;
        let processed = self.group.process_message(&self.me.provider, pm).map_err(err)?;
        let from = String::from_utf8_lossy(processed.credential().serialized_content()).into_owned();
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(a) => Ok(Received::Letter { from, body: a.into_bytes() }),
            ProcessedMessageContent::StagedCommitMessage(c) => {
                self.group.merge_staged_commit(&self.me.provider, *c).map_err(err)?;
                Ok(Received::Changed)
            }
            _ => Ok(Received::Changed),
        }
    }
}


/// Length-prefixed parts, so saved state reads back without a schema.
fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

fn take<'a>(r: &mut &'a [u8]) -> Result<&'a [u8], String> {
    if r.len() < 4 {
        return Err("saved MLS state is cut short".into());
    }
    let n = u32::from_le_bytes(r[..4].try_into().unwrap()) as usize;
    if r.len() < 4 + n {
        return Err("saved MLS state is cut short".into());
    }
    let (b, rest) = r[4..].split_at(n);
    *r = rest;
    Ok(b)
}

impl Member {
    /// Everything this PC needs to keep: its signing key, unused key
    /// packages and every group's secrets. The caller seals it before it
    /// touches the disk.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut store = Vec::new();
        for (k, v) in self.provider.storage().values.read().map_err(err)?.iter() {
            put(&mut store, k);
            put(&mut store, v);
        }
        let mut out = Vec::new();
        put(&mut out, self.peer.as_bytes());
        put(&mut out, self.signer.public());
        put(&mut out, &store);
        Ok(out)
    }

    pub fn from_bytes(raw: &[u8]) -> Result<Self, String> {
        let mut r = raw;
        let peer = String::from_utf8(take(&mut r)?.to_vec()).map_err(err)?;
        let public = take(&mut r)?.to_vec();
        let mut store = take(&mut r)?;
        let provider = OpenMlsRustCrypto::default();
        {
            let mut values = provider.storage().values.write().map_err(err)?;
            while !store.is_empty() {
                let k = take(&mut store)?.to_vec();
                values.insert(k, take(&mut store)?.to_vec());
            }
        }
        let signer = SignatureKeyPair::read(provider.storage(), &public, SUITE.signature_algorithm()).ok_or("the saved signing key is missing")?;
        let cred = CredentialWithKey { credential: BasicCredential::new(peer.as_bytes().to_vec()).into(), signature_key: public.into() };
        Ok(Member { peer, provider, signer, cred })
    }
}

impl Group {
    pub fn id(&self) -> Vec<u8> {
        self.group.group_id().as_slice().to_vec()
    }

    /// The member's whole state with this group's id in front.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        put(&mut out, &self.id());
        put(&mut out, &self.me.to_bytes()?);
        Ok(out)
    }

    pub fn from_bytes(raw: &[u8]) -> Result<Self, String> {
        let mut r = raw;
        let id = GroupId::from_slice(take(&mut r)?);
        let me = Member::from_bytes(take(&mut r)?)?;
        let group = MlsGroup::load(me.provider.storage(), &id).map_err(err)?.ok_or("the saved group is missing")?;
        Ok(Group { me, group })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_cant_read_and_removal_rekeys() {
        let mut ana = Group::create(Member::new("ana").unwrap()).unwrap();
        let bo = Member::new("bo").unwrap();
        let cy = Member::new("cy").unwrap();
        let (_, w) = ana.add(&bo.key_package().unwrap()).unwrap();
        let mut bo = Group::join(bo, &w).unwrap();
        let (commit, w) = ana.add(&cy.key_package().unwrap()).unwrap();
        assert_eq!(bo.receive(&commit).unwrap(), Received::Changed);
        let mut cy = Group::join(cy, &w).unwrap();
        assert_eq!(ana.members().len(), 3);

        let letter = ana.send(b"hello").unwrap();
        assert!(!letter.windows(5).any(|w| w == b"hello"), "the hub sees no plain text");
        assert_eq!(bo.receive(&letter).unwrap(), Received::Letter { from: "ana".into(), body: b"hello".to_vec() });
        assert!(bo.receive(&letter).is_err(), "a replay is refused");
        // A changed letter fails its AEAD check; OpenMLS debug-asserts on that,
        // so it is only refused with an error in release builds.

        let commit = ana.remove("cy").unwrap();
        assert_eq!(bo.receive(&commit).unwrap(), Received::Changed);
        let after = ana.send(b"secret").unwrap();
        assert!(cy.receive(&after).is_err(), "a removed member can't read what follows");
        assert!(matches!(bo.receive(&after).unwrap(), Received::Letter { .. }));
    }

    #[test]
    fn late_and_shuffled_letters_open() {
        let mut ana = Group::create(Member::new("ana").unwrap()).unwrap();
        let bo = Member::new("bo").unwrap();
        let (_, w) = ana.add(&bo.key_package().unwrap()).unwrap();
        let mut bo = Group::join(bo, &w).unwrap();
        let a = ana.send(b"one").unwrap();
        let b = ana.send(b"two").unwrap();
        assert!(matches!(bo.receive(&b).unwrap(), Received::Letter { .. }));
        assert!(matches!(bo.receive(&a).unwrap(), Received::Letter { .. }));
        // Sent before a change, opened after it.
        let late = ana.send(b"late").unwrap();
        let cy = Member::new("cy").unwrap();
        let (commit, _) = ana.add(&cy.key_package().unwrap()).unwrap();
        assert_eq!(bo.receive(&commit).unwrap(), Received::Changed);
        assert_eq!(bo.receive(&late).unwrap(), Received::Letter { from: "ana".into(), body: b"late".to_vec() });
    }

    #[test]
    fn saved_groups_and_members_read_back() {
        let mut ana = Group::create(Member::new("ana").unwrap()).unwrap();
        let bo = Member::new("bo").unwrap();
        let kp = bo.key_package().unwrap();
        // Saved between handing out the key package and the welcome.
        let bo = Member::from_bytes(&bo.to_bytes().unwrap()).unwrap();
        let (_, w) = ana.add(&kp).unwrap();
        let bo = Group::join(bo, &w).unwrap();
        let mut bo = Group::from_bytes(&bo.to_bytes().unwrap()).unwrap();
        let mut ana = Group::from_bytes(&ana.to_bytes().unwrap()).unwrap();
        assert_eq!(ana.id(), bo.id());
        let letter = ana.send(b"after a restart").unwrap();
        assert_eq!(bo.receive(&letter).unwrap(), Received::Letter { from: "ana".into(), body: b"after a restart".to_vec() });
        let back = bo.send(b"hi").unwrap();
        assert!(matches!(ana.receive(&back).unwrap(), Received::Letter { .. }));
        assert!(Group::from_bytes(b"\x01\x00").is_err());
    }
}
