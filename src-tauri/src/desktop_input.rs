// SPDX-License-Identifier: AGPL-3.0-only
//! Clicks, scrolling, keys and typing on macOS and Linux, for the screen
//! agent and for typing into other programs (Windows uses SendInput in
//! `programs.rs`). Each goes through a tool the system has, so nothing
//! native is linked:
//! - macOS: `osascript`. Clicks and scrolling post CGEvents from JavaScript
//!   for Automation; keys and text go through System Events. macOS asks once
//!   to allow SulcusAI under Privacy & Security > Accessibility.
//! - Linux: `xdotool` on X11, `ydotool` on Wayland (its daemon must run).

use std::process::Command;

/// The tool that sends input here, or why there's none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Osascript,
    Xdotool,
    Ydotool,
}

pub const NO_TOOL: &str = "Using the screen needs xdotool (X11) or ydotool (Wayland). Install one, then try again.";

/// Which tool to use: on Linux by session (Wayland or X11) and what's installed.
pub fn tool() -> Result<Tool, String> {
    if cfg!(target_os = "macos") {
        return Ok(Tool::Osascript);
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    pick_linux(wayland, |t| crate::engine::on_path(t).is_some()).ok_or_else(|| NO_TOOL.to_string())
}

/// Wayland apps don't take xdotool's events (only XWayland ones do), so
/// ydotool comes first there.
pub fn pick_linux(wayland: bool, has: impl Fn(&str) -> bool) -> Option<Tool> {
    let order = if wayland { [Tool::Ydotool, Tool::Xdotool] } else { [Tool::Xdotool, Tool::Ydotool] };
    order.into_iter().find(|t| has(match t {
        Tool::Xdotool => "xdotool",
        Tool::Ydotool => "ydotool",
        Tool::Osascript => "osascript",
    }))
}

/// One key combination ("ctrl+shift+t", "Enter") in each tool's terms.
/// Ctrl means Command on macOS, as shortcuts written for Windows expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub mods: Vec<&'static str>,
    pub key: String,
}

pub fn parse_combo(keys: &str) -> Result<Combo, String> {
    let parts: Vec<String> = keys.split('+').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts.split_last().ok_or("No key was given.")?;
    let mods = mods
        .iter()
        .map(|m| match m.as_str() {
            "ctrl" | "control" | "cmd" | "command" | "meta" => Ok("ctrl"),
            "shift" => Ok("shift"),
            "alt" | "option" => Ok("alt"),
            "win" | "super" => Ok("super"),
            other => Err(format!("\"{other}\" isn't a key that's held.")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Combo { mods, key: key.clone() })
}

/// xdotool's name for a key (also what ydotool's key names follow below).
fn x_key(key: &str) -> String {
    match key {
        "enter" | "return" => "Return".into(),
        "esc" | "escape" => "Escape".into(),
        "tab" => "Tab".into(),
        "space" => "space".into(),
        "backspace" => "BackSpace".into(),
        "delete" | "del" => "Delete".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "pgup" => "Prior".into(),
        "pagedown" | "pgdn" => "Next".into(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_uppercase(),
        k => k.to_string(),
    }
}

/// Linux input-event codes for ydotool (`/usr/include/linux/input-event-codes.h`).
fn y_code(key: &str) -> Option<u16> {
    let letters = "qwertyuiop\0\0\0\0asdfghjkl";
    Some(match key {
        "ctrl" => 29,
        "shift" => 42,
        "alt" => 56,
        "super" => 125,
        "enter" | "return" => 28,
        "esc" | "escape" => 1,
        "tab" => 15,
        "space" => 57,
        "backspace" => 14,
        "delete" | "del" => 111,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "home" => 102,
        "end" => 107,
        "pageup" | "pgup" => 104,
        "pagedown" | "pgdn" => 109,
        "z" => 44,
        "x" => 45,
        "c" => 46,
        "v" => 47,
        "b" => 48,
        "n" => 49,
        "m" => 50,
        k if k.len() == 1 && k.as_bytes()[0].is_ascii_digit() => match k.as_bytes()[0] {
            b'0' => 11,
            d => (d - b'1' + 2) as u16,
        },
        k if k.len() == 1 => 16 + letters.find(k)? as u16,
        k if k.starts_with('f') => match k[1..].parse::<u16>().ok()? {
            n @ 1..=10 => 58 + n,
            11 => 87,
            12 => 88,
            _ => return None,
        },
        _ => return None,
    })
}

/// macOS key codes for keys System Events can't type as text.
fn mac_code(key: &str) -> Option<u16> {
    Some(match key {
        "enter" | "return" => 36,
        "tab" => 48,
        "space" => 49,
        "backspace" | "delete" | "del" => 51,
        "esc" | "escape" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "home" => 115,
        "end" => 119,
        "pageup" | "pgup" => 116,
        "pagedown" | "pgdn" => 121,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        _ => return None,
    })
}

/// A string as an AppleScript / JavaScript literal.
fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The command line that presses `combo`.
pub fn key_command(tool: Tool, combo: &Combo) -> Result<(String, Vec<String>), String> {
    match tool {
        Tool::Xdotool => {
            let mut all: Vec<String> = combo.mods.iter().map(|m| m.to_string()).collect();
            all.push(x_key(&combo.key));
            Ok(("xdotool".into(), vec!["key".into(), "--clearmodifiers".into(), all.join("+")]))
        }
        Tool::Ydotool => {
            let codes: Vec<u16> = combo.mods.iter().map(|m| y_code(m)).chain([y_code(&combo.key)]).collect::<Option<_>>().ok_or_else(|| format!("\"{}\" can't be pressed with ydotool.", combo.key))?;
            let mut args = vec!["key".to_string()];
            args.extend(codes.iter().map(|c| format!("{c}:1")));
            args.extend(codes.iter().rev().map(|c| format!("{c}:0")));
            Ok(("ydotool".into(), args))
        }
        Tool::Osascript => {
            let using: Vec<&str> = combo
                .mods
                .iter()
                .map(|m| match *m {
                    "ctrl" => "command down",
                    "shift" => "shift down",
                    "alt" => "option down",
                    _ => "control down",
                })
                .collect();
            let using = if using.is_empty() { String::new() } else { format!(" using {{{}}}", using.join(", ")) };
            let press = match mac_code(&combo.key) {
                Some(code) => format!("key code {code}{using}"),
                None if combo.key.chars().count() == 1 => format!("keystroke {}{using}", quoted(&combo.key)),
                None => return Err(format!("\"{}\" isn't a key macOS knows by that name.", combo.key)),
            };
            Ok(("osascript".into(), vec!["-e".into(), format!("tell application \"System Events\" to {press}")]))
        }
    }
}

/// The command line that types `text` as it is.
pub fn type_command(tool: Tool, text: &str) -> (String, Vec<String>) {
    match tool {
        Tool::Xdotool => ("xdotool".into(), vec!["type".into(), "--clearmodifiers".into(), "--delay".into(), "8".into(), "--".into(), text.into()]),
        Tool::Ydotool => ("ydotool".into(), vec!["type".into(), "--".into(), text.into()]),
        Tool::Osascript => ("osascript".into(), vec!["-e".into(), format!("tell application \"System Events\" to keystroke {}", quoted(text))]),
    }
}

/// The command lines that click at a screen point.
pub fn click_commands(tool: Tool, x: i32, y: i32, right: bool, double: bool) -> Vec<(String, Vec<String>)> {
    let times = if double { 2 } else { 1 };
    match tool {
        Tool::Xdotool => vec![(
            "xdotool".into(),
            vec!["mousemove".into(), "--sync".into(), x.to_string(), y.to_string(), "click".into(), "--repeat".into(), times.to_string(), if right { "3" } else { "1" }.into()],
        )],
        Tool::Ydotool => {
            let button = if right { "0xC1" } else { "0xC0" };
            let mut click = vec!["click".into()];
            if double {
                click.extend(["--repeat".into(), "2".into()]);
            }
            click.push(button.into());
            vec![
                ("ydotool".into(), vec!["mousemove".into(), "--absolute".into(), "-x".into(), x.to_string(), "-y".into(), y.to_string()]),
                ("ydotool".into(), click),
            ]
        }
        Tool::Osascript => {
            let (down, up, button) = if right { ("kCGEventRightMouseDown", "kCGEventRightMouseUp", "kCGMouseButtonRight") } else { ("kCGEventLeftMouseDown", "kCGEventLeftMouseUp", "kCGMouseButtonLeft") };
            let script = format!(
                "ObjC.import('CoreGraphics'); var p = $.CGPointMake({x}, {y}); \
                 $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent(null, $.kCGEventMouseMoved, p, $.{button})); \
                 for (var i = 1; i <= {times}; i++) {{ ['{down}', '{up}'].forEach(function (t) {{ var e = $.CGEventCreateMouseEvent(null, $[t], p, $.{button}); \
                 $.CGEventSetIntegerValueField(e, $.kCGMouseEventClickState, i); $.CGEventPost($.kCGHIDEventTap, e); }}); }}"
            );
            vec![("osascript".into(), vec!["-l".into(), "JavaScript".into(), "-e".into(), script])]
        }
    }
}

/// The command lines that scroll at a screen point, by mouse-wheel notches.
pub fn scroll_commands(tool: Tool, x: i32, y: i32, down: bool, notches: u32) -> Vec<(String, Vec<String>)> {
    match tool {
        Tool::Xdotool => vec![(
            "xdotool".into(),
            vec!["mousemove".into(), "--sync".into(), x.to_string(), y.to_string(), "click".into(), "--repeat".into(), notches.to_string(), if down { "5" } else { "4" }.into()],
        )],
        Tool::Ydotool => vec![
            ("ydotool".into(), vec!["mousemove".into(), "--absolute".into(), "-x".into(), x.to_string(), "-y".into(), y.to_string()]),
            ("ydotool".into(), vec!["mousemove".into(), "--wheel".into(), "-x".into(), "0".into(), "-y".into(), format!("{}", if down { -(notches as i32) } else { notches as i32 })]),
        ],
        Tool::Osascript => {
            let lines = if down { -(notches as i32) * 3 } else { notches as i32 * 3 };
            let script = format!(
                "ObjC.import('CoreGraphics'); $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent(null, $.kCGEventMouseMoved, $.CGPointMake({x}, {y}), $.kCGMouseButtonLeft)); \
                 $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateScrollWheelEvent(null, $.kCGScrollEventUnitLine, 1, {lines}));"
            );
            vec![("osascript".into(), vec!["-l".into(), "JavaScript".into(), "-e".into(), script])]
        }
    }
}

/// The front window's program and title.
pub fn front_command(tool: Tool) -> (String, Vec<String>) {
    match tool {
        Tool::Osascript => (
            "osascript".into(),
            vec![
                "-e".into(),
                "tell application \"System Events\" to set p to first application process whose frontmost is true".into(),
                "-e".into(),
                "tell application \"System Events\" to set t to \"\"".into(),
                "-e".into(),
                "try\ntell application \"System Events\" to set t to name of front window of p\nend try".into(),
                "-e".into(),
                "tell application \"System Events\" to return (name of p) & linefeed & t".into(),
            ],
        ),
        // ydotool can't read windows; xdotool can when XWayland or X11 has one.
        _ => ("xdotool".into(), vec!["getactivewindow".into(), "getwindowclassname".into(), "getwindowname".into()]),
    }
}

/// Runs command lines in turn; the first failure stops them.
pub fn run(cmds: &[(String, Vec<String>)]) -> Result<(), String> {
    for (prog, args) in cmds {
        let out = Command::new(prog).args(args).output().map_err(|e| format!("Couldn't run {prog}: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(if prog == "osascript" && (err.contains("1002") || err.contains("not allowed") || err.contains("assistive")) {
                "Allow SulcusAI under System Settings > Privacy & Security > Accessibility, then try again.".to_string()
            } else {
                format!("{prog} didn't work: {}", err.trim())
            });
        }
    }
    Ok(())
}

pub fn click(x: i32, y: i32, right: bool, double: bool) -> Result<(), String> {
    run(&click_commands(tool()?, x, y, right, double))
}

pub fn scroll(x: i32, y: i32, down: bool, notches: u32) -> Result<(), String> {
    run(&scroll_commands(tool()?, x, y, down, notches))
}

pub fn keys(keys: &str) -> Result<(), String> {
    let t = tool()?;
    for combo in keys.split_whitespace() {
        run(&[key_command(t, &parse_combo(combo)?)?])?;
    }
    Ok(())
}

pub fn type_text(text: &str, enter: bool) -> Result<(), String> {
    let t = tool()?;
    if !text.is_empty() {
        run(&[type_command(t, text)])?;
    }
    if enter {
        run(&[key_command(t, &parse_combo("enter")?)?])?;
    }
    Ok(())
}

/// (program, title) of the front window; empty when it can't be read.
pub fn front() -> (String, String) {
    let Ok(t) = tool() else { return Default::default() };
    if t == Tool::Ydotool && crate::engine::on_path("xdotool").is_none() {
        return Default::default();
    }
    let (prog, args) = front_command(t);
    let Ok(out) = Command::new(prog).args(args).output() else { return Default::default() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    (lines.next().unwrap_or_default().trim().to_string(), lines.next().unwrap_or_default().trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_become_each_tools_keys() {
        let c = parse_combo("Ctrl+Shift+T").unwrap();
        assert_eq!(c, Combo { mods: vec!["ctrl", "shift"], key: "t".into() });
        assert_eq!(key_command(Tool::Xdotool, &c).unwrap().1, vec!["key", "--clearmodifiers", "ctrl+shift+t"]);
        assert_eq!(key_command(Tool::Ydotool, &c).unwrap().1, vec!["key", "29:1", "42:1", "20:1", "20:0", "42:0", "29:0"]);
        let mac = key_command(Tool::Osascript, &c).unwrap().1;
        assert!(mac[1].ends_with("keystroke \"t\" using {command down, shift down}"), "{mac:?}");
        assert_eq!(key_command(Tool::Xdotool, &parse_combo("Enter").unwrap()).unwrap().1[2], "Return");
        assert!(key_command(Tool::Osascript, &parse_combo("enter").unwrap()).unwrap().1[1].ends_with("key code 36"));
        assert_eq!(y_code("a"), Some(30));
        assert_eq!(y_code("1"), Some(2));
        assert_eq!(y_code("0"), Some(11));
        assert_eq!(y_code("f12"), Some(88));
        assert!(parse_combo("hyper+x").is_err());
    }

    #[test]
    fn typed_text_is_quoted_not_run() {
        let (_, args) = type_command(Tool::Osascript, "say \"hi\" & quit");
        assert!(args[1].ends_with(r#"keystroke "say \"hi\" & quit""#), "{args:?}");
        let (_, args) = type_command(Tool::Xdotool, "-rf");
        assert_eq!(args.last().unwrap(), "-rf");
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn wayland_prefers_ydotool() {
        let both = |_: &str| true;
        assert_eq!(pick_linux(true, both), Some(Tool::Ydotool));
        assert_eq!(pick_linux(false, both), Some(Tool::Xdotool));
        assert_eq!(pick_linux(true, |t| t == "xdotool"), Some(Tool::Xdotool));
        assert_eq!(pick_linux(false, |_| false), None);
        let clicks = click_commands(Tool::Ydotool, 10, 20, false, true);
        assert_eq!(clicks[1].1, vec!["click", "--repeat", "2", "0xC0"]);
    }
}
