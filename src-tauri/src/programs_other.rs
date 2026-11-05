// SPDX-License-Identifier: AGPL-3.0-only
//! Reading and working other programs off Windows. A small script walks the
//! window's accessibility tree (macOS: System Events, which needs the
//! Accessibility permission; Linux: AT-SPI through python3-gi) and prints it
//! as JSON; elements are numbered here the way UI Automation's are on
//! Windows. Presses and typing go to the element's middle through the
//! system's input tools (`desktop_input`), since the scripts have no handle
//! to act through.

use serde::Deserialize;

use super::{file_in_title, Window, UNTRUSTED};

/// One element as a script prints it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct El {
    pub role: String,
    /// macOS subrole ("AXSecureTextField").
    pub sub: String,
    pub name: String,
    pub value: Option<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub checked: Option<bool>,
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tree {
    pub title: String,
    pub program: String,
    pub els: Vec<El>,
}

/// A window as the list script prints it.
#[derive(Debug, Clone, Deserialize)]
pub struct Listed {
    pub pid: u32,
    pub index: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub program: String,
}

/// Window ids carry the process and the window's place in it.
pub fn window_id(pid: u32, index: u32) -> i64 {
    pid as i64 * 100 + index.min(99) as i64
}

pub fn split_id(id: i64) -> (u32, u32) {
    ((id / 100) as u32, (id % 100) as u32)
}

/// What an accessibility role is called here, and whether it gets a number,
/// for macOS AX roles and AT-SPI role names alike.
pub fn kind_of_role(role: &str, sub: &str) -> Option<(&'static str, bool)> {
    if sub == "AXSecureTextField" || role == "password text" {
        return Some(("password field", true));
    }
    Some(match role {
        "AXButton" | "AXMenuButton" | "push button" | "toggle button" | "button" => ("button", true),
        "AXCheckBox" | "check box" | "check menu item" => ("checkbox", true),
        "AXPopUpButton" | "AXComboBox" | "combo box" => ("dropdown", true),
        "AXTextField" | "AXSearchField" | "entry" | "text" | "editbar" => ("text field", true),
        "AXTextArea" | "document text" | "document web" | "document frame" | "terminal" => ("document", true),
        "AXLink" | "link" => ("link", true),
        "AXMenuItem" | "AXMenuBarItem" | "menu item" | "menu" => ("menu item", true),
        "AXRadioButton" | "radio button" | "radio menu item" => ("option", true),
        "AXSlider" | "slider" => ("slider", true),
        "AXIncrementor" | "spin button" => ("spinner", true),
        "AXTab" | "page tab" => ("tab", true),
        "AXRow" | "AXOutlineRow" | "table row" => ("row", true),
        "AXCell" | "list item" | "table cell" => ("list item", true),
        "tree item" => ("tree item", true),
        "AXStaticText" | "label" | "static" | "heading" | "paragraph" => ("text", false),
        _ => return None,
    })
}

/// A numbered element kept for pressing it later.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub kind: &'static str,
    pub name: String,
    pub password: bool,
    pub enabled: bool,
    /// The middle of the element on screen.
    pub at: (i32, i32),
}

fn clip(s: &str, n: usize) -> String {
    let s = s.replace(['\r', '\n'], " ");
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

const MAX_CHARS: usize = 24_000;

/// The window as text for the model, and its numbered elements.
pub fn render(window: &Window, els: &[El]) -> (String, Vec<Seen>) {
    let mut lines: Vec<String> = Vec::new();
    let mut seen: Vec<Seen> = Vec::new();
    let mut chars = 0usize;
    let mut more = false;
    for e in els {
        if e.visible == Some(false) || (e.w <= 0.0 && e.h <= 0.0 && e.role != "AXStaticText") {
            continue;
        }
        let Some((kind, numbered)) = kind_of_role(&e.role, &e.sub) else { continue };
        let name = clip(&e.name, 120);
        let line = if !numbered {
            let text = if name.is_empty() { clip(e.value.as_deref().unwrap_or(""), 400) } else { clip(&e.name, 400) };
            if text.is_empty() || lines.last().is_some_and(|l| l.contains(&text)) {
                continue;
            }
            text
        } else {
            let password = kind == "password field";
            let enabled = e.enabled.unwrap_or(true);
            let n = seen.len() + 1;
            let mut line = format!("[{n}] {kind}");
            if !name.is_empty() {
                line.push_str(&format!(" “{name}”"));
            }
            if !password && matches!(kind, "text field" | "dropdown" | "spinner" | "slider") {
                if let Some(v) = e.value.as_deref().map(|v| clip(v, 300)).filter(|v| !v.is_empty()) {
                    line.push_str(&format!(": “{v}”"));
                }
            }
            if kind == "checkbox" {
                let on = e.checked.unwrap_or_else(|| e.value.as_deref() == Some("1"));
                line.push_str(if on { " (on)" } else { " (off)" });
            }
            if matches!(kind, "option" | "tab" | "list item" | "tree item" | "row") && e.selected == Some(true) {
                line.push_str(" (selected)");
            }
            if !enabled {
                line.push_str(" (greyed out)");
            }
            if password {
                line.push_str(" (the user types this)");
            }
            if kind == "document" && !password {
                for l in e.value.as_deref().unwrap_or("").lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).take(80) {
                    line.push_str(&format!("\n    > {}", clip(l, 300)));
                }
            }
            seen.push(Seen { kind, name: e.name.trim().to_string(), password, enabled, at: ((e.x + e.w / 2.0) as i32, (e.y + e.h / 2.0) as i32) });
            line
        };
        chars += line.len();
        if chars > MAX_CHARS {
            more = true;
            break;
        }
        lines.push(line);
    }
    let mut text = format!("{UNTRUSTED}\n\n{} window “{}” (window {})\n", window.program, window.title, window.id);
    if let Some(file) = file_in_title(&window.title) {
        text.push_str(&format!("(The user's file “{file}” is open here. To write something new, start a new document first, e.g. program_key ctrl+n.)\n"));
    }
    if lines.is_empty() {
        text.push_str("(Nothing in it can be read: the program doesn't describe its window. Try keys with program_key.)\n");
    }
    for l in &lines {
        text.push_str(l);
        text.push('\n');
    }
    if more {
        text.push_str("(There's more than this: scroll, or open the part you need.)\n");
    }
    (text, seen)
}

/// Lists the windows (`list`) or reads one (`read PID INDEX`) as JSON.
pub const MAC_SCRIPT: &str = r#"
function run(argv) {
  const se = Application('System Events');
  if (argv[0] === 'list') {
    const r = [];
    for (const p of se.processes.whose({ backgroundOnly: false })()) {
      let ws; try { ws = p.windows(); } catch (e) { continue; }
      const pid = p.unixId(), prog = p.name();
      ws.forEach((w, i) => { let t = ''; try { t = w.name() || ''; } catch (e) {} r.push({ pid: pid, index: i, title: t, program: prog }); });
    }
    return JSON.stringify(r);
  }
  const p = se.processes.whose({ unixId: Number(argv[1]) })[0];
  const w = p.windows[Number(argv[2])];
  const out = []; let n = 0;
  function walk(e, d) {
    if (d > 14 || n > 1500) return;
    let kids; try { kids = e.uiElements(); } catch (x) { return; }
    for (const c of kids) {
      n++;
      const o = {};
      try { o.role = c.role(); } catch (x) { continue; }
      try { o.sub = c.subrole() || ''; } catch (x) {}
      try { o.name = c.name() || c.description() || ''; } catch (x) { o.name = ''; }
      try { const v = c.value(); if (v !== null && v !== undefined && typeof v !== 'object') o.value = String(v); } catch (x) {}
      try { const at = c.position(), s = c.size(); o.x = at[0]; o.y = at[1]; o.w = s[0]; o.h = s[1]; } catch (x) {}
      try { o.enabled = c.enabled(); } catch (x) {}
      try { o.selected = c.selected(); } catch (x) {}
      out.push(o);
      walk(c, d + 1);
    }
  }
  walk(w, 0);
  let title = ''; try { title = w.name() || ''; } catch (x) {}
  return JSON.stringify({ title: title, program: p.name(), els: out });
}
"#;

/// The same for AT-SPI.
pub const LINUX_SCRIPT: &str = r#"
import sys, json, gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi
d = Atspi.get_desktop(0)
def apps():
    for i in range(d.get_child_count()):
        a = d.get_child_at_index(i)
        if a is not None:
            yield a
if sys.argv[1] == 'list':
    r = []
    for a in apps():
        try:
            pid = a.get_process_id()
        except Exception:
            continue
        for j in range(min(a.get_child_count(), 99)):
            w = a.get_child_at_index(j)
            if w is not None:
                r.append({'pid': pid, 'index': j, 'title': w.get_name() or '', 'program': a.get_name() or ''})
    print(json.dumps(r))
    sys.exit(0)
pid, idx = int(sys.argv[2]), int(sys.argv[3])
app = next(a for a in apps() if a.get_process_id() == pid)
w = app.get_child_at_index(idx)
out = []
n = [0]
S = Atspi.StateType
def walk(e, dep):
    if dep > 14 or n[0] > 1500:
        return
    for k in range(e.get_child_count()):
        c = e.get_child_at_index(k)
        if c is None:
            continue
        n[0] += 1
        o = {'role': c.get_role_name() or '', 'name': c.get_name() or ''}
        try:
            st = c.get_state_set()
            o['enabled'] = st.contains(S.ENABLED) or st.contains(S.SENSITIVE)
            o['visible'] = st.contains(S.SHOWING)
            if st.contains(S.CHECKABLE) or o['role'] in ('check box', 'toggle button'):
                o['checked'] = st.contains(S.CHECKED)
            o['selected'] = st.contains(S.SELECTED)
        except Exception:
            pass
        try:
            r = c.get_extents(Atspi.CoordType.SCREEN)
            o.update(x=r.x, y=r.y, w=r.width, h=r.height)
        except Exception:
            pass
        try:
            t = c.get_text_iface()
            if t is not None:
                o['value'] = t.get_text(0, min(t.get_character_count(), 6000))
        except Exception:
            pass
        out.append(o)
        walk(c, dep + 1)
walk(w, 0)
print(json.dumps({'title': w.get_name() or '', 'program': app.get_name() or '', 'els': out}))
"#;

#[cfg(not(windows))]
pub(super) mod run {
    use super::super::{find_window, Action, Pick, Reading, Selection, Spot, Target, Window};
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    const NO_TREE_MAC: &str = "Reading other programs needs the Accessibility permission: System Settings > Privacy & Security > Accessibility, turn on SulcusAI.";
    const NO_TREE_LINUX: &str = "Reading other programs needs AT-SPI and python3-gi (on most desktops: the at-spi2-core and python3-gi packages), with accessibility turned on.";

    fn last() -> &'static Mutex<HashMap<String, (i64, Vec<Seen>)>> {
        static LAST: OnceLock<Mutex<HashMap<String, (i64, Vec<Seen>)>>> = OnceLock::new();
        LAST.get_or_init(Default::default)
    }

    async fn script(args: &[String]) -> Result<Vec<u8>, String> {
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = tokio::process::Command::new("osascript");
            c.args(["-l", "JavaScript", "-e", MAC_SCRIPT]);
            c
        } else {
            let mut c = tokio::process::Command::new("python3");
            c.args(["-I", "-c", LINUX_SCRIPT]);
            c
        };
        cmd.args(args).kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(20), cmd.output()).await.map_err(|_| "Reading the window took too long.".to_string())?.map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            crate::applog::line("agent", &format!("Reading a program failed: {}", err.trim()));
            return Err(if cfg!(target_os = "macos") { NO_TREE_MAC } else { NO_TREE_LINUX }.into());
        }
        Ok(out.stdout)
    }

    pub async fn list() -> Result<Vec<Window>, String> {
        let raw = script(&["list".into()]).await?;
        let listed: Vec<Listed> = serde_json::from_slice(&raw).map_err(|_| "Couldn't read the list of windows.".to_string())?;
        let me = std::process::id();
        Ok(listed.into_iter().filter(|w| w.pid != me && !w.title.is_empty()).map(|w| Window { id: window_id(w.pid, w.index), title: w.title, program: w.program }).collect())
    }

    async fn read_id(chat: &str, id: i64) -> Result<Reading, String> {
        let (pid, index) = split_id(id);
        if pid == std::process::id() {
            return Err("That's SulcusAI's own window, which the assistant doesn't use.".into());
        }
        let raw = script(&["read".into(), pid.to_string(), index.to_string()]).await.map_err(|e| if e.starts_with("Reading the window") { e } else { format!("That window couldn't be read (closed?). {e}") })?;
        let tree: Tree = serde_json::from_slice(&raw).map_err(|_| "Couldn't read that window.".to_string())?;
        let window = Window { id, title: tree.title, program: tree.program };
        let (text, seen) = render(&window, &tree.els);
        last().lock().unwrap().insert(chat.to_string(), (id, seen));
        Ok(Reading { window, text })
    }

    pub async fn read(chat: &str, pick: Pick) -> Result<Reading, String> {
        let id = match pick {
            Pick::Id(id) => id,
            Pick::Named(name) => find_window(&list().await?, &name).map(|w| w.id).ok_or_else(|| format!("No open window matches “{name}”. List the windows, or open the program first."))?,
            Pick::Last => last().lock().unwrap().get(chat).map(|l| l.0).ok_or("Say which window: list the windows first, or open a program.")?,
        };
        read_id(chat, id).await
    }

    fn seen(chat: &str, r: u32) -> Result<(i64, Seen), String> {
        let map = last().lock().unwrap();
        let (id, list) = map.get(chat).ok_or("Read the program's window first (program_read).")?;
        let s = list.get((r as usize).wrapping_sub(1)).ok_or_else(|| format!("There's no number {r} in the window as last read. Read it again."))?;
        Ok((*id, s.clone()))
    }

    pub async fn target(chat: &str, r: u32) -> Result<Target, String> {
        let (id, s) = seen(chat, r)?;
        let title = list().await.ok().and_then(|l| l.into_iter().find(|w| w.id == id));
        Ok(Target {
            kind: s.kind,
            name: s.name,
            password: s.password,
            program: title.as_ref().map(|w| w.program.clone()).unwrap_or_default(),
            file: title.and_then(|w| file_in_title(&w.title)),
        })
    }

    /// Brings the window's program to the front so clicks and keys reach it.
    async fn front(id: i64) {
        let (pid, _) = split_id(id);
        if cfg!(target_os = "macos") {
            let s = format!("tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true");
            let _ = tokio::process::Command::new("osascript").args(["-e", &s]).output().await;
        } else {
            let _ = tokio::process::Command::new("xdotool").args(["search", "--pid", &pid.to_string(), "windowactivate"]).output().await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    pub async fn act(chat: &str, action: Action) -> Result<(String, Reading), String> {
        let blocking = |f: Box<dyn FnOnce() -> Result<(), String> + Send>| async move { tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())? };
        let (id, done) = match &action {
            Action::Click(r) => {
                let (id, s) = seen(chat, *r)?;
                if !s.enabled {
                    return Err("That one is greyed out.".into());
                }
                front(id).await;
                let (x, y) = s.at;
                blocking(Box::new(move || crate::desktop_input::click(x, y, false, false))).await?;
                (id, format!("Pressed {}{}", s.kind, if s.name.is_empty() { format!(" [{r}]") } else { format!(" “{}”", clip(&s.name, 60)) }))
            }
            Action::Type { r, text, submit, replace } => {
                let (id, s) = seen(chat, *r)?;
                if s.password {
                    return Err("The user types passwords themselves.".into());
                }
                let replace = replace.unwrap_or(s.kind != "document");
                front(id).await;
                let ((x, y), t, enter) = (s.at, text.clone(), *submit);
                blocking(Box::new(move || {
                    crate::desktop_input::click(x, y, false, false)?;
                    crate::desktop_input::keys(if replace { "ctrl+a" } else { "ctrl+end" })?;
                    crate::desktop_input::type_text(&t, enter)
                }))
                .await?;
                let how = if replace { "Typed" } else { "Added" };
                (id, format!("{how} “{}”{}{}", clip(text, 40), if replace { "" } else { " at the end" }, if *submit { " and pressed Enter" } else { "" }))
            }
            Action::Key(keys) => {
                let id = last().lock().unwrap().get(chat).map(|l| l.0).ok_or("Read the program's window first (program_read).")?;
                front(id).await;
                let k = keys.clone();
                blocking(Box::new(move || crate::desktop_input::keys(&k))).await?;
                (id, format!("Pressed {keys}"))
            }
            Action::Scroll { r, down, screens } => {
                let (id, at) = match r {
                    Some(r) => {
                        let (id, s) = seen(chat, *r)?;
                        (id, s.at)
                    }
                    None => {
                        let map = last().lock().unwrap();
                        let (id, list) = map.get(chat).ok_or("Read the program's window first (program_read).")?;
                        (*id, list.iter().find(|s| s.kind == "document").or(list.first()).map(|s| s.at).ok_or("There's nothing in that window to scroll.")?)
                    }
                };
                front(id).await;
                let (down, notches) = (*down, screens.max(&1) * 5);
                blocking(Box::new(move || crate::desktop_input::scroll(at.0, at.1, down, notches))).await?;
                (id, format!("Scrolled {}", if down { "down" } else { "up" }))
            }
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        let reading = read_id(chat, id).await?;
        Ok((done, reading))
    }

    /// Starts a program by name and reads its first new window.
    pub async fn open(chat: &str, name: &str) -> Result<Reading, String> {
        let before: Vec<i64> = list().await.unwrap_or_default().iter().map(|w| w.id).collect();
        let started = if cfg!(target_os = "macos") {
            tokio::process::Command::new("open").args(["-a", name]).status().await
        } else {
            let id = name.to_lowercase().replace(' ', "-");
            tokio::process::Command::new("gtk-launch").arg(&id).status().await
        };
        if !started.is_ok_and(|s| s.success()) {
            return Err(format!("There's no program called “{name}” here."));
        }
        let wanted = name.to_lowercase();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let now = list().await?;
            let fresh = now.iter().find(|w| !before.contains(&w.id));
            let same = || now.iter().find(|w| w.title.to_lowercase().contains(&wanted) || w.program.to_lowercase().contains(&wanted));
            if let Some(w) = fresh.or_else(same) {
                tokio::time::sleep(Duration::from_millis(700)).await;
                return read(chat, Pick::Id(w.id)).await;
            }
        }
        Err(format!("{name} started, but no window came up yet. List the windows in a moment."))
    }

    pub fn banner_ready(_w: &tauri::WebviewWindow) {}
    pub fn at_point(_x: i32, _y: i32) -> Option<Spot> {
        None
    }
    fn logged(r: Result<(), String>) {
        if let Err(e) = r {
            crate::applog::line("agent", &e);
        }
    }
    pub fn click_at(x: i32, y: i32, right: bool, double: bool) {
        logged(crate::desktop_input::click(x, y, right, double));
    }
    pub fn scroll_at(x: i32, y: i32, down: bool, notches: u32) {
        logged(crate::desktop_input::scroll(x, y, down, notches));
    }
    pub fn keys_now(keys: &str) -> Result<(), String> {
        crate::desktop_input::keys(keys)
    }
    pub fn type_now(text: &str, enter: bool) {
        logged(crate::desktop_input::type_text(text, enter));
    }
    pub fn put_text(text: &str) {
        logged(crate::desktop_input::type_text(text, false));
    }
    pub fn front_window() -> (String, String) {
        crate::desktop_input::front()
    }
    pub fn selection_now() -> Option<Selection> {
        None
    }
    pub fn paste_into(_window: isize, _text: &str) -> Result<(), String> {
        Err("Putting an answer back is on Windows only for now.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str) -> El {
        El { role: role.into(), name: name.into(), x: 10.0, y: 20.0, w: 100.0, h: 30.0, ..Default::default() }
    }

    #[test]
    fn mac_and_atspi_roles_get_the_same_kinds() {
        assert_eq!(kind_of_role("AXButton", ""), Some(("button", true)));
        assert_eq!(kind_of_role("push button", ""), Some(("button", true)));
        assert_eq!(kind_of_role("AXTextField", "AXSecureTextField"), Some(("password field", true)));
        assert_eq!(kind_of_role("password text", ""), Some(("password field", true)));
        assert_eq!(kind_of_role("label", ""), Some(("text", false)));
        assert_eq!(kind_of_role("filler", ""), None);
    }

    #[test]
    fn render_numbers_controls_and_keeps_their_middles() {
        let w = Window { id: window_id(4321, 0), title: "notes.txt - TextEdit".into(), program: "TextEdit".into() };
        let mut check = el("check box", "Wrap");
        check.checked = Some(true);
        let mut doc = el("AXTextArea", "");
        doc.value = Some("first line\n\nsecond".into());
        let mut pw = el("AXTextField", "Password");
        pw.sub = "AXSecureTextField".into();
        pw.value = Some("secret".into());
        let mut hidden = el("push button", "Ghost");
        hidden.visible = Some(false);
        let els = vec![el("AXStaticText", "Hello"), el("AXButton", "Save"), check, doc, pw, hidden];
        let (text, seen) = render(&w, &els);
        assert!(text.starts_with(UNTRUSTED));
        assert!(text.contains("Hello\n[1] button “Save”\n[2] checkbox “Wrap” (on)\n[3] document\n    > first line\n    > second\n[4] password field “Password” (the user types this)"));
        assert!(!text.contains("secret") && !text.contains("Ghost"));
        assert!(text.contains("“notes.txt” is open here"));
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[0].at, (60, 35));
        assert!(seen[3].password);
    }

    #[test]
    fn ids_carry_the_process_and_window() {
        assert_eq!(split_id(window_id(98765, 3)), (98765, 3));
    }

    #[test]
    fn list_and_tree_json_parse() {
        let l: Vec<Listed> = serde_json::from_str(r#"[{"pid":5,"index":1,"title":"A","program":"B"}]"#).unwrap();
        assert_eq!((l[0].pid, l[0].index), (5, 1));
        let t: Tree = serde_json::from_str(r#"{"title":"T","program":"P","els":[{"role":"entry","name":"Search","enabled":false,"value":null}]}"#).unwrap();
        assert_eq!(t.els[0].enabled, Some(false));
    }
}
