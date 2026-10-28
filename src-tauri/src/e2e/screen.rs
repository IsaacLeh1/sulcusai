// SPDX-License-Identifier: AGPL-3.0-only
//! Working other programs and the screen, typing and pasting into them.

use super::*;

/// Programs: the open windows are listed, and each is read through UI
/// Automation as numbered items (until one has some). Reads only: no window
/// is opened and nothing is pressed or typed; only counts are printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_program_window_is_read() {
    let list = crate::programs::list().await.unwrap();
    println!("{} windows open: {:?}", list.len(), list.iter().map(|w| w.program.as_str()).collect::<Vec<_>>());
    assert!(!list.is_empty());
    let mut read_one = false;
    for w in &list {
        let started = std::time::Instant::now();
        let Ok(r) = crate::programs::read("test", crate::programs::Pick::Id(w.id)).await else { continue };
        let numbered = r.text.lines().filter(|l| l.starts_with('[')).count();
        println!("{}: {numbered} numbered items, {} lines, {} ms", w.program, r.text.lines().count(), started.elapsed().as_millis());
        if numbered > 0 {
            let t = crate::programs::target("test", 1).await.unwrap();
            assert!(!t.kind.is_empty());
            read_one = true;
            break;
        }
    }
    assert!(read_one, "some window has numbered items");
    assert!(crate::programs::read("test", crate::programs::Pick::Named("no such program at all".into())).await.is_err());
}

/// The screen agent's eyes, on a made-up desktop (C:\dev\tools\screen-test\
/// save-box.png: Notepad's "Do you want to save changes" box, drawn by
/// scripts in the scratchpad, buttons Save 690-860, Don't save 880-1050,
/// Cancel 1070-1240 across, 560-610 down, on 1920x1080): it points at the
/// right button for two tasks. Nothing on the real screen is touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_the_screen_agent_points_at_the_right_button() {
    let state = media_state();
    for reader in ["qwen3-vl-4b", "qwen3.5-9b"] {
        let dir = state.paths.models.join(reader);
        let Some(file) = std::fs::read_dir(&dir).ok().into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj")) else { continue };
        let size = std::fs::metadata(dir.join(&file)).unwrap().len();
        crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.into(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    }
    assert!(crate::vision::can_read(&state, None), "a picture reader is here");
    let screen = image::open(r"C:\dev\tools\screen-test\save-box.png").unwrap().to_rgba8();
    let inside = |s: &crate::screen_agent::Step, (x0, x1): (f64, f64)| match s.act {
        crate::screen_agent::Act::Click { x, y, .. } => {
            let (px, py) = (x / 1000.0 * 1920.0, y / 1000.0 * 1080.0);
            println!("  clicks at ({px:.0}, {py:.0}) for: {}", s.why);
            px >= x0 && px <= x1 && (560.0..=610.0).contains(&py)
        }
        _ => {
            println!("  not a click: {:?}", s.act);
            false
        }
    };
    let started = std::time::Instant::now();
    let (name, s) = crate::screen_agent::decide(&state, None, "Close this box without saving the file.", screen.clone()).await.unwrap();
    println!("{name}, {:.0} s:", started.elapsed().as_secs_f64());
    let right_one = inside(&s, (880.0, 1050.0));
    let (_, c) = crate::screen_agent::decide(&state, None, "Press Cancel.", screen).await.unwrap();
    let cancel = inside(&c, (1070.0, 1240.0));
    state.engine.lock().await.stop().await;
    assert!(right_one && cancel, "it pointed at Don't save and at Cancel");
}

/// The screen agent on the real screen (it moves the mouse and types):
/// asked to add a line to a Notepad file of its own and save it. The pointer
/// goes to Notepad's monitor first, since that's the one it sees.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_the_screen_agent_types_and_saves_in_notepad() {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetCursorPos};
    let state = media_state();
    for reader in ["qwen3-vl-4b", "qwen3.5-9b"] {
        let dir = state.paths.models.join(reader);
        let Some(file) = std::fs::read_dir(&dir).ok().into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj")) else { continue };
        let size = std::fs::metadata(dir.join(&file)).unwrap().len();
        crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.into(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    }
    let chat = "screen-agent-test";
    let (file, opened) = notepad_on(chat, "screen-agent-test", "Shopping list\r\n").await;
    // The agent sees the monitor the pointer is on.
    let mut r = RECT::default();
    unsafe {
        GetWindowRect(HWND(opened.window.id as _), &mut r).unwrap();
        SetCursorPos((r.left + r.right) / 2, r.top + 20).unwrap();
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let outcome = crate::screen_agent::run(
        &state,
        chat,
        None,
        "In the Notepad window with screen-agent-test.txt, click at the end of the text, add a new line that says: milk, eggs, bread. Then save the file with Ctrl+S.",
        &cancel,
        |title, detail| async move {
            println!("asked: {title}\n{detail}");
            false
        },
        |s| println!("  {s}"),
    )
    .await;
    println!("{:.0} s: {outcome:?}", started.elapsed().as_secs_f64());
    state.engine.lock().await.stop().await;
    let saved = std::fs::read_to_string(&file).unwrap();
    println!("the file: {saved:?}");
    notepad_closed(chat, "screen-agent-test", &file).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(saved.to_lowercase().contains("milk, eggs, bread"), "the line was added and saved: {saved:?}");
}

/// Notepad on a scratch file holding `text`, so the live tests never type
/// into a tab that was already open.
#[cfg(windows)]
async fn notepad_on(chat: &str, name: &str, text: &str) -> (std::path::PathBuf, crate::programs::Reading) {
    use crate::programs::{self, Pick};
    let file = std::env::temp_dir().join(format!("{name}.txt"));
    std::fs::write(&file, text).unwrap();
    std::process::Command::new("notepad.exe").arg(&file).spawn().unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let opened = programs::read(chat, Pick::Named(name.into())).await.unwrap();
    println!("opened: {}", opened.window.label());
    assert!(opened.window.title.contains(name), "our file's tab is in front: {}", opened.window.title);
    (file, opened)
}

/// Saves what's in our tab (Ctrl+S) and says what the file holds then.
#[cfg(windows)]
async fn notepad_saved(chat: &str, name: &str, file: &std::path::Path) -> String {
    use crate::programs::{self, Action, Pick};
    let front = programs::read(chat, Pick::Named(name.into())).await.unwrap();
    assert!(front.window.title.contains(name), "our tab before saving: {}", front.window.title);
    programs::act(chat, Action::Key("ctrl+s".into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    std::fs::read_to_string(file).unwrap()
}

/// Our tab closed (whatever is left unsaved isn't saved) and the file gone.
#[cfg(windows)]
async fn notepad_closed(chat: &str, name: &str, file: &std::path::Path) {
    use crate::programs::{self, Action, Pick};
    if let Ok(now) = programs::read(chat, Pick::Named(name.into())).await {
        if now.window.title.contains(name) {
            let (_, asked) = programs::act(chat, Action::Key("ctrl+w".into())).await.unwrap();
            if let Some(n) = asked.text.lines().find(|l| l.contains("Don't save") || l.contains("Don’t save")).and_then(|l| l.strip_prefix('[')).and_then(|l| l.split(']').next()).and_then(|n| n.parse::<u32>().ok()) {
                programs::act(chat, Action::Click(n)).await.ok();
            }
        } else {
            println!("not closed, another tab is in front: {}", now.window.label());
        }
    }
    std::fs::remove_file(file).ok();
}

/// Dictating into another app, live (types into Notepad): synthesized speech
/// streamed in as a microphone delivers it, each phrase put where the cursor
/// is the way Ctrl+Alt+D does it. The microphone and the hotkey aren't part
/// of it.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_dictation_types_into_notepad() {
    use std::sync::atomic::AtomicBool;
    let state = speech_state();
    let spec = state.catalog.speech.models.iter().find(|m| m.id == "whisper-small").unwrap().clone();
    crate::speech::install_for_test(&state, &spec, &AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Live).await.unwrap();
    let speak = |text: &'static str| {
        let s = crate::tts::synthesize(text, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let mut stream = vec![0.0005f32; 16_000];
    stream.extend(speak("First, buy oat milk and coffee filters."));
    stream.extend(vec![0.0005f32; 24_000]);
    stream.extend(speak("Second, book a table for four on Saturday."));
    stream.extend(vec![0.0005f32; 8_000]);

    let chat = "dictation-test";
    let (file, _) = notepad_on(chat, "dictation-test", "").await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let feeder = tokio::spawn(async move {
        for chunk in stream.chunks(320) {
            tx.send(chunk.to_vec()).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    });
    let stop = AtomicBool::new(false);
    let clipboard = crate::quick::clipboard_text();
    let emit = |kind: &str, v: serde_json::Value| {
        if kind == "text" {
            println!("  heard: {}", v["text"]);
            crate::voice::type_phrase(&v);
        }
    };
    let (r, _) = tokio::join!(crate::voice::dictate(&ep, rx, &stop, &|| 0.0, &emit), feeder);
    crate::speech::stop(&state).await;
    let after = crate::quick::clipboard_text();
    let saved = notepad_saved(chat, "dictation-test", &file).await;
    println!("the file: {saved:?}");
    notepad_closed(chat, "dictation-test", &file).await;
    r.unwrap();
    assert_eq!(saved, "First, buy oat milk and coffee filters. Second, book a table for four on Saturday. ");
    assert_eq!(after, clipboard, "the clipboard is put back");
}

/// Typing keeps every letter, live (types into Notepad): the screen agent
/// types key by key.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_typing_keeps_every_letter() {
    let chat = "typing-test";
    let (file, _) = notepad_on(chat, "typing-test", "").await;
    let line = "First, buy oat milk and coffee filters. Second, book a table for four on Saturday. The quick brown fox jumps over the lazy dog, 1234567890! ";
    for _ in 0..3 {
        crate::programs::type_now(line, false);
    }
    let saved = notepad_saved(chat, "typing-test", &file).await;
    notepad_closed(chat, "typing-test", &file).await;
    let want = line.repeat(3);
    if saved != want {
        let at = saved.chars().zip(want.chars()).position(|(a, b)| a != b).unwrap_or(saved.len().min(want.len()));
        panic!("{} of {} letters; first difference at {at}: {:?} for {:?}", saved.chars().count(), want.chars().count(), saved.chars().skip(at.saturating_sub(10)).take(30).collect::<String>(), want.chars().skip(at.saturating_sub(10)).take(30).collect::<String>());
    }
}

/// Quick ask on a selection, live (types into Notepad): the selection read
/// the way the hotkey reads it, and an answer put in its place.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_quick_ask_replaces_the_selection() {
    use crate::programs::{self, Action};
    let chat = "quick-ask-test";
    let (file, _) = notepad_on(chat, "quick-ask-test", "Keep this line.\r\nteh quick brwon fox").await;
    // The second line selected: to the end, then back to the line's start.
    programs::act(chat, Action::Key("ctrl+end".into())).await.unwrap();
    programs::act(chat, Action::Key("shift+home".into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let clipboard = crate::quick::clipboard_text();
    let sel = programs::selection_now();
    println!("selected: {sel:?}");
    let sel = sel.expect("the selection is read");
    assert_eq!(sel.text, "teh quick brwon fox");
    assert!(sel.editable, "Notepad's text can take the answer");
    tokio::task::spawn_blocking(move || programs::paste_into(sel.window, "The quick brown fox.")).await.unwrap().unwrap();
    let after = crate::quick::clipboard_text();
    let saved = notepad_saved(chat, "quick-ask-test", &file).await;
    println!("the file: {saved:?}");
    notepad_closed(chat, "quick-ask-test", &file).await;
    assert_eq!(saved.replace("\r\n", "\n"), "Keep this line.\nThe quick brown fox.");
    assert_eq!(after, clipboard, "the clipboard is put back");
}

/// Programs, live (drives the mouse and keyboard): text added in a new
/// Notepad tab, then the tab closed without saving. It stops before typing
/// unless the tab is a new one, since Notepad reopens the files that were
/// open last time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_notepad_is_typed_into() {
    use crate::programs::{self, Action, Pick};
    let chat = "notepad-test";
    let opened = programs::open(chat, "Notepad").await.unwrap();
    println!("opened: {}", opened.window.label());
    let (done, fresh) = programs::act(chat, Action::Key("ctrl+n".into())).await.unwrap();
    println!("{done}: {}", fresh.window.label());
    assert!(fresh.window.title.starts_with("Untitled"), "a new tab before typing: {}", fresh.window.title);
    let doc = fresh.text.lines().find(|l| l.contains("document")).and_then(|l| l.strip_prefix('[')).and_then(|l| l.split(']').next()).and_then(|n| n.parse::<u32>().ok()).expect("a document to type into");
    let (typed, after) = programs::act(chat, Action::Type { r: doc, text: "Hello from SulcusAI".into(), submit: false, replace: None }).await.unwrap();
    println!("{typed}");
    assert!(after.text.contains("Hello from SulcusAI"), "{}", after.text);
    // The tab closed without saving it.
    let (_, asked) = programs::act(chat, Action::Key("ctrl+w".into())).await.unwrap();
    println!("closing: {}", asked.window.label());
    if let Some(n) = asked.text.lines().find(|l| l.contains("Don't save") || l.contains("Don’t save")).and_then(|l| l.strip_prefix('[')).and_then(|l| l.split(']').next()).and_then(|n| n.parse::<u32>().ok()) {
        let (pressed, _) = programs::act(chat, Action::Click(n)).await.unwrap();
        println!("{pressed}");
    }
    // Not Alt+F4: the window has other tabs. An empty tab that opening
    // Notepad made closes without asking.
    if let Ok(r) = programs::read(chat, Pick::Named("Notepad".into())).await {
        println!("in front after: {}", r.window.label());
        assert!(!r.window.title.starts_with('*') && !r.window.title.contains("Hello from SulcusAI"), "the typed tab is gone: {}", r.window.title);
        if r.window.title == "Untitled - Notepad" {
            programs::act(chat, Action::Key("ctrl+w".into())).await.ok();
        }
    }
}
