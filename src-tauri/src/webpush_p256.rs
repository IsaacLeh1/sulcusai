// SPDX-License-Identifier: AGPL-3.0-only
//! The real P-256 behind `webpush::Curve`, from the RustCrypto `p256` crate.
//! Kept apart because that crate has to be a direct dependency of the app
//! (`p256 = { version = "0.13", features = ["ecdh", "ecdsa"] }`); it is
//! already in the lock file through the Tor client.

use aes_gcm::aead::OsRng;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use zeroize::Zeroizing;

use crate::webpush::Curve;

pub struct P256;

fn secret(private: &[u8; 32]) -> Result<SecretKey, String> {
    SecretKey::from_slice(private).map_err(|_| "Not a valid P-256 private key.".to_string())
}

fn point(key: &PublicKey) -> Result<[u8; 65], String> {
    key.to_encoded_point(false).as_bytes().try_into().map_err(|_| "Unexpected public key size.".to_string())
}

impl Curve for P256 {
    fn generate(&self) -> Result<(Zeroizing<[u8; 32]>, [u8; 65]), String> {
        let sk = SecretKey::random(&mut OsRng);
        let mut private = Zeroizing::new([0u8; 32]);
        private.copy_from_slice(&sk.to_bytes());
        Ok((private, point(&sk.public_key())?))
    }

    fn public_of(&self, private: &[u8; 32]) -> Result<[u8; 65], String> {
        point(&secret(private)?.public_key())
    }

    fn ecdh(&self, private: &[u8; 32], peer_public: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
        let peer = PublicKey::from_sec1_bytes(peer_public).map_err(|_| "The phone's push key isn't a valid P-256 point.".to_string())?;
        let shared = p256::ecdh::diffie_hellman(secret(private)?.to_nonzero_scalar(), peer.as_affine());
        let mut out = Zeroizing::new([0u8; 32]);
        out.copy_from_slice(shared.raw_secret_bytes());
        Ok(out)
    }

    fn sign(&self, private: &[u8; 32], message: &[u8]) -> Result<[u8; 64], String> {
        let sig: Signature = SigningKey::from(&secret(private)?).sign(message);
        Ok(sig.to_bytes().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webpush::{encrypt_with, BrowserSubscription, PushStore, Vapid};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use base64::Engine;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;

    fn unb64(s: &str) -> Vec<u8> {
        B64URL.decode(s).unwrap()
    }

    // RFC 8291, section 5.
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const AS_PUBLIC: &str = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const ECDH_SECRET: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn key(b64: &str) -> [u8; 32] {
        unb64(b64).try_into().unwrap()
    }

    #[test]
    fn rfc8291_vector_with_real_curve_maths() {
        assert_eq!(P256.public_of(&key(AS_PRIVATE)).unwrap().to_vec(), unb64(AS_PUBLIC));
        assert_eq!(P256.public_of(&key(UA_PRIVATE)).unwrap().to_vec(), unb64(UA_PUBLIC));
        assert_eq!(P256.ecdh(&key(AS_PRIVATE), &unb64(UA_PUBLIC)).unwrap().to_vec(), unb64(ECDH_SECRET));
        assert_eq!(P256.ecdh(&key(UA_PRIVATE), &unb64(AS_PUBLIC)).unwrap().to_vec(), unb64(ECDH_SECRET));

        let mut store = PushStore::default();
        let sub: BrowserSubscription = serde_json::from_value(serde_json::json!({
            "endpoint": "https://fcm.googleapis.com/fcm/send/abc", "keys": { "p256dh": UA_PUBLIC, "auth": AUTH }
        }))
        .unwrap();
        store.subscribe("p1", &sub, 0).unwrap();
        let body = encrypt_with(&P256, &key(AS_PRIVATE), &unb64(SALT).try_into().unwrap(), &store.subs[0], b"When I grow up, I want to be a watermelon").unwrap();
        assert_eq!(B64URL.encode(body), BODY);
    }

    #[test]
    fn vapid_jwt_signature_verifies_against_the_public_key() {
        let v = Vapid::generate(&P256, "mailto:push@example.com").unwrap();
        let jwt = v.jwt(&P256, "https://fcm.googleapis.com", 1_700_000_000).unwrap();
        let (signing_input, sig) = jwt.rsplit_once('.').unwrap();
        let public = VerifyingKey::from_sec1_bytes(&unb64(&v.public_b64())).unwrap();
        let sig = Signature::from_slice(&unb64(sig)).unwrap();
        public.verify(signing_input.as_bytes(), &sig).unwrap();
        assert!(public.verify(b"another.input", &sig).is_err());
    }

    #[test]
    fn generated_pairs_agree_on_the_shared_secret() {
        let (a, a_pub) = P256.generate().unwrap();
        let (b, b_pub) = P256.generate().unwrap();
        assert_eq!(*P256.ecdh(&a, &b_pub).unwrap(), *P256.ecdh(&b, &a_pub).unwrap());
        assert!(P256.ecdh(&a, &[4u8; 65]).is_err());
    }
}
