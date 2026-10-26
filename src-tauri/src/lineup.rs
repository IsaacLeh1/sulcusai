// SPDX-License-Identifier: AGPL-3.0-only
//! The line for the graphics cards. Chat replies and Studio jobs wait their
//! turn rather than running beside each other and pushing work onto the
//! processor. Replies from the chat model already running share it; other
//! work goes on beside what's running only when the card's memory holds
//! both; the rest waits, in order. With a second card, work that runs there
//! (a song's parts, a voice sung again) takes it while the main card is
//! busy. The person sees the line at the bottom of the window, can move
//! what's waiting, and can start something now: on the processor, or by
//! stopping what's in its way.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;

use crate::{download, AppState, AppStateRef};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Card {
    Main,
    Second,
}

/// Something in the line.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: String,
    /// "chat" or "studio".
    pub kind: String,
    /// The chat's title, or what the job was asked for.
    pub title: String,
    /// What it does ("A reply", "Making the picture").
    pub what: String,
    pub chat_id: Option<String>,
    /// The chat model a reply uses: replies with the same one share it.
    pub model: Option<String>,
    /// The graphics memory it needs (bytes); 0 when unknown (it then runs
    /// only on its own).
    pub need: u64,
    /// It can run on the second card.
    pub second_ok: bool,
    /// It can run on the processor instead, when started now.
    pub processor_ok: bool,
    pub running: bool,
    /// Where it runs, once running: a card, or None for the processor.
    pub card: Option<Card>,
    /// Part of a running reply (a picture or song a chat makes).
    pub inside: Option<String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    #[serde(skip)]
    cancel: Option<Arc<AtomicBool>>,
    /// Started now: first in line, on the processor when `card` stays None.
    #[serde(skip)]
    now: Option<How>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum How {
    Processor,
    Next,
}

impl Entry {
    pub fn chat(chat_id: &str, title: &str, model: &str, need: u64) -> Entry {
        Entry::new("chat", &format!("chat:{chat_id}:{}", uuid::Uuid::new_v4().simple()), title, "A reply", need).with_chat(chat_id, Some(model))
    }

    pub fn studio(job_id: &str, title: &str, what: &str, need: u64) -> Entry {
        Entry::new("studio", job_id, title, what, need)
    }

    fn new(kind: &str, id: &str, title: &str, what: &str, need: u64) -> Entry {
        Entry {
            id: id.into(),
            kind: kind.into(),
            title: title.chars().take(80).collect(),
            what: what.into(),
            chat_id: None,
            model: None,
            need,
            second_ok: false,
            processor_ok: false,
            running: false,
            card: None,
            inside: None,
            queued_at: crate::db::now_ms(),
            started_at: None,
            cancel: None,
            now: None,
        }
    }

    pub fn with_chat(mut self, chat_id: &str, model: Option<&str>) -> Entry {
        self.chat_id = Some(chat_id.into());
        self.model = model.map(str::to_string);
        self
    }

    pub fn second_ok(mut self, ok: bool) -> Entry {
        self.second_ok = ok;
        self
    }

    pub fn processor_ok(mut self, ok: bool) -> Entry {
        self.processor_ok = ok;
        self
    }
}

static LINE: LazyLock<Mutex<Vec<Entry>>> = LazyLock::new(Default::default);
static CHANGED: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

fn changed(state: &AppState) {
    CHANGED.notify_waiters();
    if let Some(app) = state.app.get() {
        app.emit("lineup:changed", list()).ok();
    }
}

/// The line now: what's running first, then what waits, in order.
pub fn list() -> Vec<Entry> {
    let line = LINE.lock().unwrap();
    line.iter().filter(|e| e.running).chain(line.iter().filter(|e| !e.running)).cloned().collect()
}

/// The graphics memory each card offers (bytes): the main one's as models
/// are fitted to it, the second's most of it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Room {
    pub main: u64,
    pub second: Option<u64>,
}

fn room(state: &AppState) -> Room {
    let second = state.hardware.read().unwrap().second_gpu().map(|g| g.vram_bytes / 10 * 9);
    Room { main: state.budget().vram_card, second }
}

/// Where the entry at `i` can start now, if it can: a card, or the
/// processor (`Some(None)`, started now that way).
fn may_start(line: &[Entry], i: usize, room: Room) -> Option<Option<Card>> {
    let e = &line[i];
    if e.now == Some(How::Processor) {
        return Some(None);
    }
    for card in [Card::Main, Card::Second] {
        let total = match card {
            Card::Main => room.main,
            Card::Second if e.second_ok => match room.second {
                Some(r) => r,
                None => continue,
            },
            Card::Second => continue,
        };
        let here: Vec<&Entry> = line.iter().filter(|r| r.running && r.card == Some(card) && r.inside.is_none()).collect();
        // Replies from the chat model running share it.
        let shares = e.kind == "chat" && !here.is_empty() && here.iter().all(|r| r.kind == "chat" && r.model == e.model);
        // Beside what's running when the memory holds both. One chat
        // model runs at a time (one engine), and unknown needs run alone.
        let beside = !here.is_empty()
            && e.need > 0
            && here.iter().all(|r| r.need > 0)
            && !(e.kind == "chat" && here.iter().any(|r| r.kind == "chat"))
            && here.iter().map(|r| r.need).sum::<u64>() + e.need <= total;
        if !(here.is_empty() || shares || beside) {
            continue;
        }
        // In turn: nothing waiting before it could take this card (unless
        // it shares a running model, or was started now).
        let ahead = line[..i].iter().any(|x| !x.running && (card == Card::Main || x.second_ok));
        if ahead && !shares && e.now.is_none() {
            continue;
        }
        return Some(Some(card));
    }
    None
}

/// The turn, held while the work runs; letting go of it frees its place.
pub struct Turn {
    id: String,
    state: Arc<AppState>,
    /// Where it runs: a card, or None for the processor.
    pub card: Option<Card>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        LINE.lock().unwrap().retain(|e| e.id != self.id);
        changed(&self.state);
    }
}

/// Waits for the entry's turn (`on_wait` hears its place: 1 is next).
/// `cancel` ends the wait (Stop, or taken out of the line).
pub async fn wait(state: &Arc<AppState>, mut e: Entry, cancel: Arc<AtomicBool>, mut on_wait: impl FnMut(usize)) -> Result<Turn, String> {
    let id = e.id.clone();
    e.cancel = Some(cancel.clone());
    LINE.lock().unwrap().push(e);
    changed(state);
    // Held from here, so leaving the wait (Stop) takes it out of the line.
    let mut turn = Turn { id: id.clone(), state: state.clone(), card: None };
    let mut last_place = 0;
    loop {
        let notified = CHANGED.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let started = {
            let mut line = LINE.lock().unwrap();
            let i = line.iter().position(|x| x.id == id).ok_or(download::CANCELLED)?;
            match may_start(&line, i, room(state)) {
                Some(card) => {
                    line[i].running = true;
                    line[i].card = card;
                    line[i].started_at = Some(crate::db::now_ms());
                    Some(card)
                }
                None => {
                    let place = line.iter().filter(|x| !x.running).position(|x| x.id == id).map_or(1, |p| p + 1);
                    if place != last_place {
                        last_place = place;
                        on_wait(place);
                    }
                    None
                }
            }
        };
        if let Some(card) = started {
            changed(state);
            if card.is_none() {
                crate::applog::line("card", &format!("Started now on the processor (asked for): {id}"));
            }
            turn.card = card;
            return Ok(turn);
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(Duration::from_millis(400)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.into());
                }
            }
        }
    }
}

/// A Studio job a running reply makes (a picture, a song): part of that
/// reply's turn, not behind it (it would wait for itself). None when the
/// chat isn't replying.
pub fn inside_reply(state: &Arc<AppState>, chat_id: &str, mut e: Entry) -> Option<Turn> {
    let mut line = LINE.lock().unwrap();
    let reply = line.iter().find(|x| x.running && x.kind == "chat" && x.chat_id.as_deref() == Some(chat_id))?;
    e.running = true;
    e.card = reply.card;
    e.inside = Some(reply.id.clone());
    e.started_at = Some(crate::db::now_ms());
    let id = e.id.clone();
    let card = e.card;
    line.push(e);
    drop(line);
    changed(state);
    Some(Turn { id, state: state.clone(), card })
}

/// A running job moves its work to the second card when that card is free
/// and something waits for the main one (a song's voice sung again there
/// while the next song is made). True when it moved.
pub fn hand_over(state: &AppState, id: &str) -> bool {
    let room = room(state);
    if room.second.is_none() {
        return false;
    }
    let mut line = LINE.lock().unwrap();
    let second_busy = line.iter().any(|x| x.running && x.card == Some(Card::Second) && x.inside.is_none());
    let waiting = line.iter().any(|x| !x.running);
    let Some(e) = line.iter_mut().find(|x| x.id == id && x.running && x.card == Some(Card::Main)) else { return false };
    if second_busy || !waiting || !e.second_ok {
        return false;
    }
    e.card = Some(Card::Second);
    drop(line);
    changed(state);
    crate::applog::line("card", &format!("Moved {id} to the second card so the main one can take the next in line"));
    true
}

/// Where a running entry is now.
pub fn card_of(id: &str) -> Option<Card> {
    LINE.lock().unwrap().iter().find(|x| x.id == id && x.running).and_then(|x| x.card)
}

// ---------- the window's controls ----------

#[tauri::command]
pub fn lineup_list() -> Vec<Entry> {
    list()
}

/// Moves a waiting entry to `to` among the waiting ones (0 is next).
#[tauri::command]
pub fn lineup_move(state: AppStateRef, id: String, to: usize) -> Result<(), String> {
    {
        let mut line = LINE.lock().unwrap();
        let from = line.iter().position(|x| x.id == id && !x.running).ok_or("That's no longer waiting.")?;
        let e = line.remove(from);
        let waiting: Vec<usize> = line.iter().enumerate().filter(|(_, x)| !x.running).map(|(i, _)| i).collect();
        let at = waiting.get(to).copied().unwrap_or(line.len());
        line.insert(at, e);
    }
    changed(&state);
    Ok(())
}

/// What starting it now would take: whether it can run on the processor,
/// and what's running in its way (on the card it needs).
#[derive(Debug, Clone, Serialize)]
pub struct StartNow {
    pub processor: bool,
    pub in_the_way: Vec<Entry>,
}

#[tauri::command]
pub fn lineup_start_now_options(id: String) -> Result<StartNow, String> {
    let line = LINE.lock().unwrap();
    let e = line.iter().find(|x| x.id == id && !x.running).ok_or("That's no longer waiting.")?;
    let in_the_way = line.iter().filter(|x| x.running && x.inside.is_none() && (x.card == Some(Card::Main) || (e.second_ok && x.card == Some(Card::Second)))).cloned().collect();
    Ok(StartNow { processor: e.processor_ok, in_the_way })
}

/// Starts a waiting entry now: "processor" runs it there beside what's on
/// the card; "stop" stops what's in its way and makes it next.
#[tauri::command]
pub fn lineup_start_now(state: AppStateRef, id: String, how: String) -> Result<(), String> {
    {
        let mut line = LINE.lock().unwrap();
        let i = line.iter().position(|x| x.id == id && !x.running).ok_or("That's no longer waiting.")?;
        match how.as_str() {
            "processor" if line[i].processor_ok => line[i].now = Some(How::Processor),
            "processor" => return Err("This can't run on the processor.".into()),
            "stop" => {
                let second_ok = line[i].second_ok;
                for r in line.iter().filter(|x| x.running && x.inside.is_none() && (x.card == Some(Card::Main) || (second_ok && x.card == Some(Card::Second)))) {
                    if let Some(c) = &r.cancel {
                        c.store(true, Ordering::Relaxed);
                    }
                    if r.kind == "studio" {
                        crate::media::cancel(&r.id);
                    }
                }
                line[i].now = Some(How::Next);
                let e = line.remove(i);
                let first_waiting = line.iter().position(|x| !x.running).unwrap_or(line.len());
                line.insert(first_waiting, e);
            }
            _ => return Err("Unknown choice.".into()),
        }
    }
    crate::applog::line("card", &format!("Started {id} now ({how})"));
    changed(&state);
    Ok(())
}

/// Takes a waiting entry out of the line (its job or reply stops).
#[tauri::command]
pub fn lineup_remove(state: AppStateRef, id: String) -> Result<(), String> {
    let e = LINE.lock().unwrap().iter().find(|x| x.id == id).cloned().ok_or("That's no longer in line.")?;
    if let Some(c) = &e.cancel {
        c.store(true, Ordering::Relaxed);
    }
    if e.kind == "studio" {
        crate::media::cancel(&e.id);
    }
    changed(&state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn entry(kind: &str, id: &str, model: Option<&str>, need: u64) -> Entry {
        let mut e = Entry::new(kind, id, id, "", need);
        e.model = model.map(str::to_string);
        e
    }

    fn running(mut e: Entry, card: Card) -> Entry {
        e.running = true;
        e.card = Some(card);
        e
    }

    #[test]
    fn work_waits_for_the_card_unless_it_shares_or_fits() {
        let room = Room { main: 12 * GB, second: None };
        // A reply from the running chat model shares it, even behind others.
        let line = vec![running(entry("chat", "a", Some("qwen"), 6 * GB), Card::Main), entry("studio", "pic", None, 8 * GB), entry("chat", "b", Some("qwen"), 6 * GB)];
        assert_eq!(may_start(&line, 2, room), Some(Some(Card::Main)));
        // The picture doesn't fit beside the reply: it waits.
        assert_eq!(may_start(&line, 1, room), None);
        // Another chat model can't run beside the first (one engine).
        let line = vec![running(entry("chat", "a", Some("qwen"), 3 * GB), Card::Main), entry("chat", "c", Some("gemma"), 3 * GB)];
        assert_eq!(may_start(&line, 1, room), None);
        // Small work fits beside, in turn.
        let line = vec![running(entry("studio", "pic", None, 6 * GB), Card::Main), entry("chat", "a", Some("qwen"), 5 * GB)];
        assert_eq!(may_start(&line, 1, room), Some(Some(Card::Main)));
        // Unknown needs run alone.
        let line = vec![running(entry("studio", "x", None, 0), Card::Main), entry("chat", "a", Some("qwen"), 1 * GB)];
        assert_eq!(may_start(&line, 1, room), None);
        // In order: what's behind something waiting for the card waits too.
        let line = vec![running(entry("studio", "pic", None, 6 * GB), Card::Main), entry("studio", "video", None, 10 * GB), entry("chat", "a", Some("qwen"), 5 * GB)];
        assert_eq!(may_start(&line, 2, room), None);
    }

    #[test]
    fn a_second_card_takes_what_can_run_there() {
        let room = Room { main: 15 * GB, second: Some(10 * GB) };
        let mut parts = entry("studio", "parts", None, 4 * GB);
        parts.second_ok = true;
        let line = vec![running(entry("studio", "song", None, 14 * GB), Card::Main), parts.clone(), entry("studio", "pic", None, 8 * GB)];
        assert_eq!(may_start(&line, 1, room), Some(Some(Card::Second)));
        // What can't run there waits for the main card.
        assert_eq!(may_start(&line, 2, room), None);
        // The processor, when started now that way.
        let mut now = entry("studio", "pic", None, 8 * GB);
        now.now = Some(How::Processor);
        let line = vec![running(entry("studio", "song", None, 14 * GB), Card::Main), now];
        assert_eq!(may_start(&line, 1, room), Some(None));
    }
}
