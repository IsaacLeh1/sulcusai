// SPDX-License-Identifier: AGPL-3.0-only
//! Remote control on this PC: pictures of the screen for the controller and
//! their input played back. Called only while a session is live (the wire
//! layer checks `Host::may_input` before every event).

use std::sync::Mutex;

use sulcus_net::sharing::wire::Input;

/// Where the last picture came from on the desktop: (left, top, width, height).
static SCREEN: Mutex<(i32, i32, u32, u32)> = Mutex::new((0, 0, 0, 0));
const MAX_WIDTH: u32 = 1280;

/// A JPEG of the monitor the pointer is on, at most 1280 wide.
pub fn frame() -> Option<Vec<u8>> {
    let (img, (x, y)) = crate::media::screen::capture_at().ok()?;
    *SCREEN.lock().unwrap() = (x, y, img.width(), img.height());
    let img = image::DynamicImage::ImageRgba8(img);
    let img = if img.width() > MAX_WIDTH { img.resize(MAX_WIDTH, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60).encode_image(&img.to_rgb8()).ok()?;
    Some(out.into_inner())
}

/// Input lands on this part of the desktop (a shared monitor in a call).
pub fn set_screen(left: i32, top: i32, width: u32, height: u32) {
    *SCREEN.lock().unwrap() = (left, top, width, height);
}

/// A point given as fractions of the picture, on the desktop.
pub fn to_screen(fx: f32, fy: f32, screen: (i32, i32, u32, u32)) -> (i32, i32) {
    let (x, y, w, h) = screen;
    let fx = fx.clamp(0.0, 1.0);
    let fy = fy.clamp(0.0, 1.0);
    (x + (fx * w.saturating_sub(1) as f32).round() as i32, y + (fy * h.saturating_sub(1) as f32).round() as i32)
}

/// Browser `KeyboardEvent.code` to a Windows virtual key.
pub fn vk(code: &str) -> Option<u16> {
    if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1) {
        return c.bytes().next().filter(u8::is_ascii_uppercase).map(u16::from);
    }
    if let Some(d) = code.strip_prefix("Digit").filter(|c| c.len() == 1) {
        return d.bytes().next().filter(u8::is_ascii_digit).map(u16::from);
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some(0x6F + n);
    }
    Some(match code {
        "Enter" | "NumpadEnter" => 0x0D,
        "Escape" => 0x1B,
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Space" => 0x20,
        "ShiftLeft" | "ShiftRight" => 0x10,
        "ControlLeft" | "ControlRight" => 0x11,
        "AltLeft" | "AltRight" => 0x12,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Delete" => 0x2E,
        "Insert" => 0x2D,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "CapsLock" => 0x14,
        "Minus" => 0xBD,
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "BracketLeft" => 0xDB,
        "BracketRight" => 0xDD,
        "Backslash" => 0xDC,
        "Backquote" => 0xC0,
        // The Windows keys are left out: Win+L and the like stay with the person at the PC.
        _ => return None,
    })
}

#[cfg(windows)]
pub fn apply(i: Input) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
    let mouse = |flags: MOUSE_EVENT_FLAGS, data: i32| INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
    let send = |v: &[INPUT]| unsafe {
        SendInput(v, std::mem::size_of::<INPUT>() as i32);
    };
    match i {
        Input::Move { x, y } => {
            let (sx, sy) = to_screen(x, y, *SCREEN.lock().unwrap());
            unsafe {
                let _ = SetCursorPos(sx, sy);
            }
        }
        Input::Click { button, down } => {
            let f = match (button, down) {
                (0, true) => MOUSEEVENTF_LEFTDOWN,
                (0, false) => MOUSEEVENTF_LEFTUP,
                (1, true) => MOUSEEVENTF_MIDDLEDOWN,
                (1, false) => MOUSEEVENTF_MIDDLEUP,
                (2, true) => MOUSEEVENTF_RIGHTDOWN,
                (2, false) => MOUSEEVENTF_RIGHTUP,
                _ => return,
            };
            send(&[mouse(f, 0)]);
        }
        Input::Scroll { dy } => send(&[mouse(MOUSEEVENTF_WHEEL, (-dy).clamp(-1200.0, 1200.0) as i32)]),
        Input::Key { code, down } => {
            let Some(k) = vk(&code) else { return };
            let flags = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
            let flags = if matches!(k, 0x21..=0x28 | 0x2D | 0x2E) { flags | KEYEVENTF_EXTENDEDKEY } else { flags };
            send(&[INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(k), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }]);
        }
    }
}

#[cfg(not(windows))]
pub fn apply(_: Input) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_and_windows_keys_stay_local() {
        assert_eq!(vk("KeyA"), Some(0x41));
        assert_eq!(vk("Digit7"), Some(0x37));
        assert_eq!(vk("F5"), Some(0x74));
        assert_eq!(vk("ArrowDown"), Some(0x28));
        assert_eq!(vk("MetaLeft"), None);
        assert_eq!(vk("Keya"), None);
        assert_eq!(vk("F13"), None);
    }

    #[test]
    fn points_land_on_the_shared_monitor() {
        let s = (1920, 0, 2560, 1440);
        assert_eq!(to_screen(0.0, 0.0, s), (1920, 0));
        assert_eq!(to_screen(1.0, 1.0, s), (1920 + 2559, 1439));
        assert_eq!(to_screen(2.0, -1.0, s), (1920 + 2559, 0));
    }
}
