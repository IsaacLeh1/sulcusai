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
use openmls_traits::OpenMlsProvider as _;

/// X25519, ChaCha20-Poly1305, SHA-256 and Ed25519, the same family as the
/// rest of the network code.
pub const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;

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
        let config = MlsGroupCreateConfig::builder().ciphersuite(SUITE).use_ratchet_tree_extension(true).build();
        let group = MlsGroup::new(&me.provider, &me.signer, &config, me.cred.clone()).map_err(err)?;
        Ok(Group { me, group })
    }

    /// Joins from the welcome someone sent after adding this PC.
    pub fn join(me: Member, welcome: &[u8]) -> Result<Self, String> {
        let msg = MlsMessageIn::tls_deserialize(&mut &welcome[..]).map_err(err)?;
        let MlsMessageBodyIn::Welcome(w) = msg.extract() else { return Err("not a welcome".into()) };
        let staged = StagedWelcome::new_from_welcome(&me.provider, &MlsGroupJoinConfig::default(), w, None).map_err(err)?;
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
        let mut forged = cy.send(b"x").unwrap();
        let n = forged.len() - 3;
        forged[n] ^= 1;
        assert!(bo.receive(&forged).is_err(), "a changed letter is refused");

        let commit = ana.remove("cy").unwrap();
        assert_eq!(bo.receive(&commit).unwrap(), Received::Changed);
        let after = ana.send(b"secret").unwrap();
        assert!(cy.receive(&after).is_err(), "a removed member can't read what follows");
        assert!(matches!(bo.receive(&after).unwrap(), Received::Letter { .. }));
    }
}
