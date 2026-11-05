// SPDX-License-Identifier: AGPL-3.0-only
//! Security notice keys and signatures, kept out of SulcusAI on purpose: the
//! private key must only ever touch an offline PC, so the app has no way to
//! sign anything.
//!
//!   notice-signer keygen <folder>            makes notice.key and notice.pub there
//!   notice-signer sign <notice.key> <body.json> <out.json>
//!   notice-signer check <signed.json> <public key hex>...
//!   notice-signer manifest <notice.key> <version> <setup.exe> <setup.exe.sig> <out.json> [engine files]...

use std::path::Path;
use std::process::ExitCode;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sulcus_net::manifest;
use sulcus_net::security_notice::{self, Notice, TrustState, SIGNING_DOMAIN};

fn keygen(folder: &Path) -> Result<String, String> {
    let key = folder.join("notice.key");
    if key.exists() {
        return Err(format!("{} already exists; a key is never overwritten.", key.display()));
    }
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "couldn't make a key".to_string())?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "the new key didn't load".to_string())?;
    let public = hex::encode(pair.public_key().as_ref());
    std::fs::write(&key, B64.encode(pkcs8.as_ref())).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("notice.pub"), &public).map_err(|e| e.to_string())?;
    Ok(public)
}

fn load(key: &Path) -> Result<Ed25519KeyPair, String> {
    let text = std::fs::read_to_string(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let pkcs8 = B64.decode(text.trim()).map_err(|_| "the key file isn't base64".to_string())?;
    Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| "the key file isn't an Ed25519 key".to_string())
}

/// The body is parsed first so a typo is caught here, not by every app that
/// rejects the notice later.
fn sign(pair: &Ed25519KeyPair, body: &[u8]) -> Result<String, String> {
    let notice: Notice = serde_json::from_slice(body).map_err(|e| format!("the body isn't a notice: {e}"))?;
    if notice.sequence == 0 {
        return Err("sequence starts at 1 and goes up with every notice".into());
    }
    let mut signed = SIGNING_DOMAIN.to_vec();
    signed.extend_from_slice(body);
    let sig = pair.sign(&signed);
    let envelope = serde_json::json!({
        "v": 1,
        "body": B64.encode(body),
        "key": hex::encode(pair.public_key().as_ref()),
        "sig": B64.encode(sig.as_ref()),
    });
    Ok(envelope.to_string())
}

/// The release manifest peers pass updates by: same key, its own prefix.
fn sign_manifest(pair: &Ed25519KeyPair, m: &manifest::Manifest) -> Result<String, String> {
    let body = serde_json::to_vec(m).map_err(|e| e.to_string())?;
    let mut signed = manifest::DOMAIN.to_vec();
    signed.extend_from_slice(&body);
    let sig = pair.sign(&signed);
    let envelope = serde_json::json!({ "v": 1, "body": B64.encode(&body), "key": hex::encode(pair.public_key().as_ref()), "sig": B64.encode(sig.as_ref()) });
    Ok(envelope.to_string())
}

fn describe(path: &str) -> Result<manifest::File, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path);
    Ok(manifest::describe(name, &bytes))
}

fn check(raw: &[u8], keys: &[String]) -> Result<Notice, String> {
    let keys = keys.iter().map(|k| security_notice::parse_key(k).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    security_notice::accept_with(raw, &TrustState::default(), &keys, now).map(|(n, _)| n).map_err(|e| e.to_string())
}

fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => keygen(Path::new(&args[1])).map(|p| format!("Made notice.key and notice.pub in {}.\nPublic key (goes in BUILT_IN_KEYS): {p}", args[1])),
        Some("sign") if args.len() == 4 => {
            let pair = load(Path::new(&args[1]))?;
            let body = std::fs::read(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?;
            let out = sign(&pair, &body)?;
            check(out.as_bytes(), &[hex::encode(pair.public_key().as_ref())])?;
            std::fs::write(&args[3], &out).map_err(|e| e.to_string())?;
            Ok(format!("Signed and checked: {}", args[3]))
        }
        Some("check") if args.len() >= 3 => {
            let raw = std::fs::read(&args[1]).map_err(|e| format!("{}: {e}", args[1]))?;
            let n = check(&raw, &args[2..])?;
            Ok(format!("Good: sequence {}, lowest safe version {}, {} message(s).", n.sequence, n.lowest_safe_version, n.messages.len()))
        }
        Some("manifest") if args.len() >= 6 => {
            let pair = load(Path::new(&args[1]))?;
            let sig = std::fs::read_to_string(&args[4]).map_err(|e| format!("{}: {e}", args[4]))?;
            let issued = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let m = manifest::Manifest { version: args[2].clone(), issued, installer: describe(&args[3])?, installer_sig: sig.trim().to_string(), engines: args[6..].iter().map(|p| describe(p)).collect::<Result<_, _>>()? };
            let out = sign_manifest(&pair, &m)?;
            manifest::accept(out.as_bytes(), &[pair.public_key().as_ref().try_into().map_err(|_| "bad key")?]).map_err(|e| e.to_string())?;
            std::fs::write(&args[5], &out).map_err(|e| e.to_string())?;
            Ok(format!("Signed and checked: {}", args[5]))
        }
        _ => Err("Usage:\n  notice-signer keygen <folder>\n  notice-signer sign <notice.key> <body.json> <out.json>\n  notice-signer check <signed.json> <public key hex>...
  notice-signer manifest <notice.key> <version> <setup.exe> <setup.exe.sig> <out.json> [engine files]...".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_notice_passes_the_apps_own_check() {
        let dir = std::env::temp_dir().join(format!("notice-signer-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let public = keygen(&dir).unwrap();
        assert!(keygen(&dir).is_err(), "a key is never overwritten");
        let pair = load(&dir.join("notice.key")).unwrap();
        let body = br#"{"sequence":1,"issued":1793800000,"lowest_safe_version":"0.9.49","messages":[{"severity":"info","text":"Test"}]}"#;
        let out = sign(&pair, body).unwrap();
        assert_eq!(check(out.as_bytes(), &[public.clone()]).unwrap().sequence, 1);
        let other = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
        assert!(check(out.as_bytes(), &[hex::encode(other.public_key().as_ref())]).is_err(), "another key's notice is refused");
        assert!(sign(&pair, br#"{"sequence":0,"issued":1,"lowest_safe_version":"0.9.49"}"#).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
