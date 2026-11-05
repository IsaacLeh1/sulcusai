// SPDX-License-Identifier: AGPL-3.0-only
//! Calls and screen share, hosted by members' PCs.
//!
//! Small calls connect everyone to everyone. Past that, one member's PC
//! forwards streams for the rest (it never decodes them: media stays sealed
//! with the call key). Game capture never reaches into the game's process,
//! which is what anti-cheat systems watch for.

use serde::{Deserialize, Serialize};

use super::MemberId;

/// Above this many people, everyone-to-everyone costs too much upload.
pub const MESH_MAX: usize = 4;
/// Upload one forwarded stream needs, in kbit/s (720p screen + voice).
pub const STREAM_KBPS: u32 = 2_500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: MemberId,
    pub upload_kbps: u32,
    /// Reachable without a relay (open port or same network).
    pub reachable: bool,
    /// Playing a game now; a gaming PC shouldn't also forward a call.
    pub in_game: bool,
    pub on_battery: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Layout {
    Mesh,
    Hosted { host: MemberId },
    /// No PC in the call can carry it; the reason says what to change.
    NotPossible { reason: String },
}

/// Picks how a call runs. The same inputs pick the same host on every PC, so
/// members agree without a vote.
pub fn plan(people: &[Candidate], sharing_screens: usize) -> Layout {
    if people.len() <= MESH_MAX {
        return Layout::Mesh;
    }
    let need = STREAM_KBPS * (people.len() as u32 - 1) * sharing_screens.max(1) as u32;
    let mut able: Vec<&Candidate> = people.iter().filter(|c| c.reachable && c.upload_kbps >= need).collect();
    able.sort_by(|a, b| {
        (a.in_game, a.on_battery, std::cmp::Reverse(a.upload_kbps), &a.id).cmp(&(b.in_game, b.on_battery, std::cmp::Reverse(b.upload_kbps), &b.id))
    });
    match able.first() {
        Some(c) => Layout::Hosted { host: c.id.clone() },
        None => Layout::NotPossible { reason: format!("A call this size needs one member reachable with about {} Mbit/s upload.", need.div_ceil(1000)) },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    Windowed,
    Borderless,
    ExclusiveFullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoder {
    Nvenc,
    Amf,
    QuickSync,
    Software,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum Capture {
    /// Windows Graphics Capture from outside the game.
    Window,
    /// OBS's signed game-capture module, already installed by the user.
    ObsModule,
    /// DXGI desktop duplication of the game's monitor (social::native).
    Duplication,
    /// Can't capture safely; ask the user to switch the game to borderless.
    AskBorderless,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturePlan {
    pub capture: Capture,
    pub encoder: Encoder,
    /// Only this process's sound goes into the stream.
    pub audio_pid: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Machine {
    pub gpu_vendor: String,
    pub obs_module_installed: bool,
    /// Process loopback capture needs Windows 10 2004 or newer.
    pub process_loopback: bool,
    /// The game sound switch; process loopback is used only when it's on.
    pub game_sound: bool,
    /// The native screen capture switch.
    pub native_screen_capture: bool,
}

pub fn encoder_for(vendor: &str) -> Encoder {
    let v = vendor.to_ascii_lowercase();
    if v.contains("nvidia") {
        Encoder::Nvenc
    } else if v.contains("amd") || v.contains("advanced micro") {
        Encoder::Amf
    } else if v.contains("intel") {
        Encoder::QuickSync
    } else {
        Encoder::Software
    }
}

/// How to share a game: outside the game first; the signed module only for
/// exclusive full screen; nothing that injects into the game.
pub fn capture_plan(mode: WindowMode, game_pid: u32, m: &Machine) -> CapturePlan {
    let capture = match mode {
        WindowMode::Windowed | WindowMode::Borderless => Capture::Window,
        WindowMode::ExclusiveFullscreen if m.obs_module_installed => Capture::ObsModule,
        WindowMode::ExclusiveFullscreen if m.native_screen_capture => Capture::Duplication,
        WindowMode::ExclusiveFullscreen => Capture::AskBorderless,
    };
    CapturePlan { capture, encoder: encoder_for(&m.gpu_vendor), audio_pid: (m.process_loopback && m.game_sound).then_some(game_pid) }
}

/// The anti-cheat systems capture must be checked against on real games
/// before calls ship; recorded here so the check list can't drift.
pub const ANTI_CHEAT_CHECKS: &[&str] = &["Easy Anti-Cheat", "BattlEye", "Vanguard", "FACEIT"];
