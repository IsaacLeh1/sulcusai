// SPDX-License-Identifier: AGPL-3.0-only
//! A relay of one's own: "Run a relay on this PC" starts the bundled
//! sulcus-relay in the background with this PC as its owner, and the hosting
//! guide writes the commands and cloud-init for a server in the person's own
//! Oracle or Cloudflare account. The app never signs in to or creates
//! anything in those accounts: the person runs what it writes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::AppStateRef;

const REPO: &str = "https://github.com/IsaacLeh1/sulcusai";
pub const DEFAULT_PORT: u16 = 4001;
/// Oracle's Always Free outbound limit is 10 TB a month; stop well before it.
pub const DEFAULT_CAP_GB: u64 = 8000;

fn relay_dir(data: &Path) -> PathBuf {
    data.join("relay")
}

fn relay_exe() -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "sulcus-relay.exe" } else { "sulcus-relay" };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = exe.parent().map(|p| p.join(name)).ok_or("no program folder")?;
    if path.exists() {
        Ok(path)
    } else {
        Err("The relay program (sulcus-relay) isn't installed next to SulcusAI.".into())
    }
}

/// Adds `owner` to owners.json in the relay's folder (the format the relay reads).
pub fn add_owner(dir: &Path, owner: &str) -> Result<(), String> {
    let path = dir.join("owners.json");
    let mut list: Vec<String> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !list.iter().any(|o| o == owner) {
        list.push(owner.to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(&path, serde_json::to_vec_pretty(&list).unwrap_or_default()).map_err(|e| e.to_string())
}

fn pid_of(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("relay.pid")).ok()?.trim().parse().ok()
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("sulcus-relay"))
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cmdline")).map(|c| c.contains("sulcus-relay")).unwrap_or(false)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRelay {
    pub running: bool,
    pub port: u16,
    /// The relay's network id, once it has a key.
    pub id: Option<String>,
    pub folder: String,
}

fn status(data: &Path, port: u16) -> LocalRelay {
    let dir = relay_dir(data);
    let id = std::fs::read_to_string(dir.join("relay.key"))
        .ok()
        .and_then(|h| (0..h.trim().len()).step_by(2).map(|i| h.trim().get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok())).collect::<Option<Vec<u8>>>())
        .and_then(|k| sulcus_net::identity::peer_id(&k).ok())
        .map(|p| p.to_string());
    LocalRelay { running: pid_of(&dir).is_some_and(alive), port, id, folder: dir.display().to_string() }
}

#[tauri::command]
pub fn p2p_local_relay(state: AppStateRef) -> LocalRelay {
    let s = super::settings::load(&state.db.lock().unwrap());
    status(&state.paths.data, s.local_relay_port.unwrap_or(DEFAULT_PORT))
}

/// Starts the relay as a background program that keeps running after
/// SulcusAI closes, with this PC as owner so "Your relay" can manage it.
#[tauri::command]
pub fn p2p_local_relay_start(state: AppStateRef, port: Option<u16>) -> Result<LocalRelay, String> {
    let port = port.unwrap_or(DEFAULT_PORT);
    if port < 1024 {
        return Err("Pick a port from 1024 up.".into());
    }
    let dir = relay_dir(&state.paths.data);
    if pid_of(&dir).is_some_and(alive) {
        return Ok(status(&state.paths.data, port));
    }
    let cipher = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
    let key = super::keys::node_key(&state.db.lock().unwrap(), &cipher)?;
    let me = sulcus_net::identity::peer_id(&key).map_err(|e| e.to_string())?;
    add_owner(&dir, &me.to_string())?;
    let exe = relay_exe()?;
    let log = std::fs::File::create(dir.join("relay.log")).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["run", "--port", &port.to_string(), "--dir"]).arg(&dir).stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No window, and not tied to SulcusAI's console or job.
        cmd.creation_flags(0x0800_0000 | 0x0000_0008 | 0x0000_0200);
    }
    let child = cmd.spawn().map_err(|e| format!("couldn't start the relay: {e}"))?;
    std::fs::write(dir.join("relay.pid"), child.id().to_string()).map_err(|e| e.to_string())?;
    let mut s = super::settings::load(&state.db.lock().unwrap());
    s.local_relay_port = Some(port);
    super::settings::save(&state.db.lock().unwrap(), &s)?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(status(&state.paths.data, port))
}

#[tauri::command]
pub fn p2p_local_relay_stop(state: AppStateRef) -> Result<LocalRelay, String> {
    let dir = relay_dir(&state.paths.data);
    if let Some(pid) = pid_of(&dir).filter(|p| alive(*p)) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).creation_flags(0x0800_0000).status().map_err(|e| e.to_string())?;
        }
        #[cfg(not(windows))]
        std::process::Command::new("kill").arg(pid.to_string()).status().map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(dir.join("relay.pid"));
    let s = super::settings::load(&state.db.lock().unwrap());
    Ok(status(&state.paths.data, s.local_relay_port.unwrap_or(DEFAULT_PORT)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Oracle,
    Cloudflare,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    /// Numbered steps to follow in the person's own account.
    pub steps: Vec<String>,
    /// Paste into the server's cloud-init (user data) box.
    pub cloud_init: String,
    /// Run on the person's own PC or in the provider's cloud shell.
    pub commands: String,
}

fn valid_owner(owner: &str) -> bool {
    owner.parse::<sulcus_net::libp2p::PeerId>().is_ok()
}

/// cloud-init for an Ubuntu server: builds and runs the relay as a systemd
/// service with this PC as owner, opens the port, and stops the relay for
/// the rest of the month once outbound traffic reaches the cap.
pub fn cloud_init(owner: &str, port: u16, cap_gb: u64) -> String {
    format!(
        r#"#cloud-config
package_update: true
packages: [build-essential, pkg-config, git, vnstat, iptables-persistent]
users:
  - name: sulcus
    system: true
    shell: /usr/sbin/nologin
write_files:
  - path: /etc/systemd/system/sulcus-relay.service
    content: |
      [Unit]
      Description=Sulcus Relay
      After=network-online.target
      [Service]
      User=sulcus
      ExecStart=/usr/local/bin/sulcus-relay run --port {port} --dir /var/lib/sulcus-relay
      Restart=on-failure
      [Install]
      WantedBy=multi-user.target
  - path: /usr/local/bin/sulcus-relay-cap
    permissions: "0755"
    content: |
      #!/bin/sh
      # Stops the relay once this month's outbound traffic reaches {cap_gb} GB.
      tx=$(vnstat --oneline b | cut -d';' -f10)
      if [ "${{tx:-0}}" -ge $(({cap_gb} * 1000000000)) ]; then systemctl stop sulcus-relay; fi
  - path: /etc/cron.d/sulcus-relay-cap
    content: |
      */5 * * * * root /usr/local/bin/sulcus-relay-cap
      0 0 1 * * root systemctl start sulcus-relay
runcmd:
  - iptables -I INPUT -p tcp --dport {port} -j ACCEPT
  - iptables -I INPUT -p udp --dport {port} -j ACCEPT
  - ip6tables -I INPUT -p tcp --dport {port} -j ACCEPT
  - ip6tables -I INPUT -p udp --dport {port} -j ACCEPT
  - netfilter-persistent save
  - curl -fsSL -o /usr/local/bin/sulcus-relay {REPO}/releases/latest/download/sulcus-relay-linux-$(uname -m) && chmod 755 /usr/local/bin/sulcus-relay || (curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal && git clone --depth 1 {REPO} /opt/sulcusai && /root/.cargo/bin/cargo build --release --manifest-path /opt/sulcusai/crates/sulcus-relay/Cargo.toml && install /opt/sulcusai/crates/sulcus-relay/target/release/sulcus-relay /usr/local/bin/)
  - install -d -o sulcus -m 700 /var/lib/sulcus-relay
  - sudo -u sulcus /usr/local/bin/sulcus-relay owner {owner} --dir /var/lib/sulcus-relay
  - systemctl enable --now vnstat sulcus-relay
"#
    )
}

pub fn guide(provider: Provider, owner: &str, port: u16, cap_gb: u64) -> Result<Guide, String> {
    if !valid_owner(owner) {
        return Err("Turn on Network first, so this PC has a network id to own the relay.".into());
    }
    if port < 1024 {
        return Err("Pick a port from 1024 up.".into());
    }
    let cap_gb = cap_gb.clamp(1, 9500);
    let init = cloud_init(owner, port, cap_gb);
    let check = format!("ssh ubuntu@<server address> sudo journalctl -u sulcus-relay -n 20\n# The relay's id is on the \"running as\" line. In SulcusAI: Settings > Network > Add relay:\n#   /ip4/<server address>/udp/{port}/quic-v1/p2p/<relay id>");
    Ok(match provider {
        Provider::Oracle => Guide {
            steps: vec![
                "Make your own Oracle Cloud account (Oracle's sign-up; SulcusAI never makes it).".into(),
                "Billing > Budgets: create a budget of 1 (your currency) with an alert, so any charge emails you.".into(),
                format!("Compute > Instances > Create: an Always Free shape only (VM.Standard.A1.Flex, up to 4 OCPU and 24 GB, or VM.Standard.E2.1.Micro), Ubuntu. Never pick a shape without the Always Free label."),
                "Under Advanced options > Management, paste the cloud-init below.".into(),
                format!("Networking > your subnet's Security List: add ingress rules for TCP and UDP port {port} from 0.0.0.0/0."),
                format!("The relay stops for the rest of the month at {cap_gb} GB sent (Always Free allows 10 TB). Oracle may reclaim an idle Always Free machine."),
                "To remove everything: terminate the instance (and its boot volume) in Compute > Instances.".into(),
            ],
            cloud_init: init,
            commands: format!(
                "# Optional, in Oracle Cloud Shell: a budget alert that emails you at any spend\noci budgets budget create --compartment-id <tenancy OCID> --target-type COMPARTMENT --targets '[\"<tenancy OCID>\"]' --amount 1 --reset-period MONTHLY --display-name sulcus-zero\n\n# To remove everything later (the instance and its disk):\noci compute instance terminate --instance-id <instance OCID> --preserve-boot-volume false --force\n\n{check}"
            ),
        },
        Provider::Cloudflare => Guide {
            steps: vec![
                "Cloudflare can't run the relay itself: it needs a server that takes UDP and TCP. Use your Oracle server (above) or another you run.".into(),
                "In your own Cloudflare account, add a DNS record for the server so its address stays the same if the IP changes.".into(),
                "Set the record to DNS only (grey cloud): Cloudflare's proxy doesn't pass the relay's traffic.".into(),
                "In SulcusAI, add the relay with /dns4/<your name>/... in place of /ip4/<address>/.".into(),
            ],
            cloud_init: init,
            commands: format!(
                "# With your own API token (Zone > DNS > Edit), on your own PC:\ncurl -X POST \"https://api.cloudflare.com/client/v4/zones/<zone id>/dns_records\" -H \"Authorization: Bearer <your token>\" -H \"Content-Type: application/json\" --data '{{\"type\":\"A\",\"name\":\"relay\",\"content\":\"<server address>\",\"proxied\":false}}'\n# In SulcusAI: /dns4/relay.<your domain>/udp/{port}/quic-v1/p2p/<relay id>\n\n{check}"
            ),
        },
    })
}

#[tauri::command]
pub fn p2p_hosting_guide(state: AppStateRef, provider: Provider, port: Option<u16>, cap_gb: Option<u64>) -> Result<Guide, String> {
    let s = super::settings::load(&state.db.lock().unwrap());
    if !s.own_hosting {
        return Err("Turn on hosting on your own account first.".into());
    }
    let owner = super::running().map(|n| n.peer_id().to_string()).unwrap_or_default();
    guide(provider, &owner, port.unwrap_or(DEFAULT_PORT), cap_gb.unwrap_or(DEFAULT_CAP_GB))
}

#[tauri::command]
pub fn p2p_set_own_hosting(state: AppStateRef, on: bool) -> Result<bool, String> {
    let db = state.db.lock().unwrap();
    let mut s = super::settings::load(&db);
    s.own_hosting = on;
    super::settings::save(&db, &s)?;
    Ok(on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> String {
        sulcus_net::identity::peer_id(&sulcus_net::identity::generate()).unwrap().to_string()
    }

    #[test]
    fn guide_needs_an_owner_and_a_port() {
        assert!(guide(Provider::Oracle, "", 4001, 8000).is_err());
        assert!(guide(Provider::Oracle, &owner(), 80, 8000).is_err());
    }

    #[test]
    fn oracle_guide_names_owner_port_and_cap() {
        let o = owner();
        let g = guide(Provider::Oracle, &o, 4555, 8000).unwrap();
        assert!(g.cloud_init.starts_with("#cloud-config"));
        assert!(g.cloud_init.contains(&format!("owner {o}")));
        assert!(g.cloud_init.contains("--port 4555"));
        assert!(g.cloud_init.contains("8000 * 1000000000"));
        assert!(g.steps.iter().any(|s| s.contains("Always Free")));
        let capped = guide(Provider::Cloudflare, &o, 4001, 50_000).unwrap();
        assert!(capped.cloud_init.contains("9500 * 1000000000"), "never above the free limit");
        assert!(capped.commands.contains("\"proxied\":false"));
    }

    #[test]
    fn owners_file_keeps_each_once() {
        let dir = std::env::temp_dir().join(format!("sulcus-host-test-{}", std::process::id()));
        add_owner(&dir, "a").unwrap();
        add_owner(&dir, "a").unwrap();
        add_owner(&dir, "b").unwrap();
        let list: Vec<String> = serde_json::from_slice(&std::fs::read(dir.join("owners.json")).unwrap()).unwrap();
        assert_eq!(list, ["a", "b"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Keeps the relay running at boot with nobody signed in. Needs an
/// administrator, so Windows asks for permission; the relay registers
/// itself (see `sulcus-relay install`).
#[tauri::command]
pub fn p2p_local_relay_at_boot(state: AppStateRef, on: bool) -> Result<(), String> {
    let exe = relay_exe()?;
    let dir = relay_dir(&state.paths.data);
    let port = super::settings::load(&state.db.lock().unwrap()).local_relay_port.unwrap_or(DEFAULT_PORT);
    let args = if on { format!("install --port {port} --dir \"\"{}\"\"", dir.display()) } else { "uninstall".to_string() };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let ps = format!("$p = Start-Process -FilePath '{}' -ArgumentList \"{args}\" -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode", exe.display());
        let ok = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &ps]).creation_flags(0x0800_0000).status().map_err(|e| e.to_string())?.success();
        if !ok {
            return Err("Windows didn't allow it, or the relay couldn't register.".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, args);
        Err("On Linux run: sudo sulcus-relay install".into())
    }
}
