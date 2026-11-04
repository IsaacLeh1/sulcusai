// SPDX-License-Identifier: AGPL-3.0-only
//! The Sulcus Relay: run it on a PC or server that others can reach, and
//! SulcusAI PCs that can't reach each other directly connect through it.
//!
//!   sulcus-relay run [--port 4001] [--dir <folder>]
//!   sulcus-relay id [--dir <folder>]
//!   sulcus-relay key <circle> [--dir <folder>]   (an invite key for joining)
//!   sulcus-relay owner <network id> [--dir <folder>]   (a PC that may manage it)
//!
//! The relay's key is made on the first run and kept in `relay.key` in the
//! folder (by default next to the program). It prints the addresses to give
//! people; they add one under Settings › Network › Add relay.

use std::path::PathBuf;

use sulcus_net::libp2p::core::multiaddr::Protocol;
use sulcus_net::libp2p::Multiaddr;
use sulcus_net::{identity, Node, NodeConfig, NodeEvent};

const DEFAULT_PORT: u16 = 4001;

struct Args {
    command: String,
    port: u16,
    dir: PathBuf,
    /// The circle a new invite key is for ("Family").
    label: String,
}

/// Who may join, kept next to the key. It holds invite secrets, so the
/// folder should be readable by this program only.
fn admissions(dir: &std::path::Path) -> sulcus_net::join::Admissions {
    std::fs::read(dir.join("admissions.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The PCs (network ids) that may approve, remove and make keys.
fn owners(dir: &std::path::Path) -> Vec<String> {
    std::fs::read(dir.join("owners.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_admissions(dir: &std::path::Path, a: &sulcus_net::join::Admissions) {
    let saved = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join("admissions.json"), serde_json::to_vec_pretty(a).unwrap_or_default()));
    if let Err(e) = saved {
        eprintln!("couldn't save admissions.json in {}: {e}", dir.display());
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut label = String::new();
    let mut port = DEFAULT_PORT;
    let mut dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."));
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).ok_or("--port needs a number")?,
            "--dir" => dir = PathBuf::from(args.next().ok_or("--dir needs a folder")?),
            other if label.is_empty() && !other.starts_with("--") => label = other.to_string(),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(Args { command, port, dir, label })
}

/// The relay's key, made and saved on the first run. Kept as hex in a file
/// only this program reads; anyone with it could pose as this relay.
fn key(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let path = dir.join("relay.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let bytes = hex_decode(text.trim()).ok_or("relay.key can't be read")?;
        identity::load(&bytes).map_err(|e| e.to_string())?;
        return Ok(bytes);
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = identity::generate();
    std::fs::write(&path, hex_encode(&bytes)).map_err(|e| format!("couldn't save {}: {e}", path.display()))?;
    Ok(bytes)
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

fn listen_on(port: u16) -> Vec<Multiaddr> {
    vec![
        format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse().unwrap(),
        format!("/ip4/0.0.0.0/tcp/{port}").parse().unwrap(),
        format!("/ip6/::/udp/{port}/quic-v1").parse().unwrap(),
        format!("/ip6/::/tcp/{port}").parse().unwrap(),
    ]
}

fn main() {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    match args.command.as_str() {
        "id" => match key(&args.dir).and_then(|k| identity::peer_id(&k).map_err(|e| e.to_string())) {
            Ok(id) => println!("{id}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        "key" => {
            let label = if args.label.is_empty() { "Friends".to_string() } else { args.label.clone() };
            let mut a = admissions(&args.dir);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let (id, secret) = a.rotate(&label, now);
            save_admissions(&args.dir, &a);
            println!("New invite key for \"{label}\" (older keys for it stop admitting newcomers):
  key id: {id}
  secret: {secret}");
        }
        "owner" => {
            if args.label.parse::<sulcus_net::libp2p::PeerId>().is_err() {
                eprintln!("give the owner PC's network id (Settings › Network in SulcusAI)");
                std::process::exit(2);
            }
            let mut o = owners(&args.dir);
            if !o.contains(&args.label) {
                o.push(args.label.clone());
            }
            let saved = std::fs::create_dir_all(&args.dir).and_then(|_| std::fs::write(args.dir.join("owners.json"), serde_json::to_vec_pretty(&o).unwrap_or_default()));
            match saved {
                Ok(()) => println!("{} may now manage this relay.", args.label),
                Err(e) => {
                    eprintln!("couldn't save owners.json: {e}");
                    std::process::exit(1);
                }
            }
        }
        "run" => {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
            if let Err(e) = rt.block_on(run(args)) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        _ => println!("sulcus-relay {}\n\nUsage:\n  sulcus-relay run [--port {DEFAULT_PORT}] [--dir <folder>]\n  sulcus-relay id [--dir <folder>]", env!("CARGO_PKG_VERSION")),
    }
}

async fn run(args: Args) -> Result<(), String> {
    let key = key(&args.dir)?;
    let cfg = NodeConfig { listen: listen_on(args.port), relay_server: true, ..NodeConfig::any_port(key) };
    let node = Node::start(cfg).map_err(|e| e.to_string())?;
    let adm = std::sync::Arc::new(std::sync::Mutex::new(admissions(&args.dir)));
    let node = std::sync::Arc::new(node);
    // Members and owners may relay; everyone else is cut off.
    let allow = {
        let (adm, dir, node) = (adm.clone(), args.dir.clone(), node.clone());
        move || {
            let a = adm.lock().unwrap();
            let set = a.members.iter().chain(owners(&dir).iter()).filter(|p| !a.blocked.contains(p)).filter_map(|p| p.parse().ok()).collect();
            node.set_relay_members(Some(set));
        }
    };
    allow();
    let (keep, dir) = (adm.clone(), args.dir.clone());
    let refresh = allow.clone();
    sulcus_net::join_wire::serve(&node, adm.clone(), move || {
        save_admissions(&dir, &keep.lock().unwrap());
        refresh();
    });
    let (keep, dir) = (adm.clone(), args.dir.clone());
    let owner_list = std::sync::Arc::new(std::sync::Mutex::new(owners(&args.dir)));
    sulcus_net::admin_wire::serve(&node, adm, owner_list, move || {
        save_admissions(&dir, &keep.lock().unwrap());
        allow();
    });
    let id = node.peer_id();
    println!("Sulcus Relay {} running as {id}", env!("CARGO_PKG_VERSION"));
    println!("Give people one of these (with this PC's public address in place of a private one):");
    let mut events = node.subscribe();
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(NodeEvent::Listening(a)) if !a.iter().any(|p| matches!(p, Protocol::P2pCircuit)) => println!("  {}", a.with(Protocol::P2p(id))),
                Ok(NodeEvent::Connected { peer, .. }) => println!("connected: {peer}"),
                Ok(NodeEvent::Disconnected { peer }) => println!("left: {peer}"),
                Ok(_) => {}
                Err(_) => {}
            },
            _ = tokio::signal::ctrl_c() => {
                println!("stopping");
                node.stop().await;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_keys() {
        let k = parse(["key", "Family"].into_iter().map(String::from)).unwrap();
        assert_eq!((k.command.as_str(), k.label.as_str()), ("key", "Family"));
        let a = parse(["run", "--port", "5000", "--dir", "x"].into_iter().map(String::from)).unwrap();
        assert_eq!((a.command.as_str(), a.port, a.dir), ("run", 5000, PathBuf::from("x")));
        assert!(parse(["run", "--port"].into_iter().map(String::from)).is_err());
        assert!(parse(["run", "--bogus"].into_iter().map(String::from)).is_err());
        let b = vec![0u8, 1, 254, 255];
        assert_eq!(hex_decode(&hex_encode(&b)), Some(b));
        let dir = std::env::temp_dir().join(format!("sulcus-relay-test-{}", std::process::id()));
        let k1 = key(&dir).unwrap();
        assert_eq!(key(&dir).unwrap(), k1, "the same key on the next run");
        std::fs::remove_dir_all(&dir).ok();
    }
}
