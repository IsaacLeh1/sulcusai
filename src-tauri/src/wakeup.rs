// SPDX-License-Identifier: AGPL-3.0-only
//! Waking this PC from sleep for a scheduled task, when the person turned
//! that on: a wake timer a minute before the next run (Windows wakes for it
//! when its power plan allows wake timers, which it does by default when
//! plugged in), and the PC kept from sleeping while a task runs.

use std::sync::Mutex;

use crate::{db, AppStateRef};

const KEY: &str = "wake_for_tasks";

pub fn on(conn: &rusqlite::Connection) -> bool {
    db::get(conn, KEY).unwrap_or(false)
}

/// The timer, kept open (as an integer: a handle can't cross threads).
static TIMER: Mutex<Option<isize>> = Mutex::new(None);

/// Wakes the PC at `at` (ms since 1970), or never with None. Whether
/// Windows took it.
#[cfg(windows)]
pub fn wake_at(at: Option<i64>) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer, TIMER_ALL_ACCESS};
    let mut timer = TIMER.lock().unwrap();
    // SAFETY: the handle is made here and only used under this lock.
    unsafe {
        let h = match *timer {
            Some(h) => HANDLE(h as *mut _),
            None => match CreateWaitableTimerExW(None, None, 0, TIMER_ALL_ACCESS.0) {
                Ok(h) => {
                    *timer = Some(h.0 as isize);
                    h
                }
                Err(_) => return false,
            },
        };
        match at {
            // Positive: an absolute time, in 100 ns steps since 1601.
            Some(ms) => {
                let due = (ms + 11_644_473_600_000) * 10_000;
                SetWaitableTimer(h, &due, 0, None, None, true).is_ok()
            }
            None => CancelWaitableTimer(h).is_ok(),
        }
    }
}

#[cfg(not(windows))]
pub fn wake_at(_at: Option<i64>) -> bool {
    false
}

/// Keeps the PC from sleeping until dropped.
pub struct Awake(#[allow(dead_code)] isize);

impl Awake {
    #[cfg(windows)]
    pub fn new(why: &str) -> Option<Awake> {
        use windows::core::PWSTR;
        use windows::Win32::System::Power::{PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
        use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};
        let mut text: Vec<u16> = why.encode_utf16().chain(std::iter::once(0)).collect();
        let reason = REASON_CONTEXT { Version: 0, Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING, Reason: REASON_CONTEXT_0 { SimpleReasonString: PWSTR(text.as_mut_ptr()) } };
        // SAFETY: `reason` and its text live through both calls.
        unsafe {
            let h = PowerCreateRequest(&reason).ok()?;
            PowerSetRequest(h, PowerRequestSystemRequired).ok()?;
            Some(Awake(h.0 as isize))
        }
    }

    #[cfg(not(windows))]
    pub fn new(_why: &str) -> Option<Awake> {
        None
    }
}

#[cfg(windows)]
impl Drop for Awake {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::Power::{PowerClearRequest, PowerRequestSystemRequired};
        // SAFETY: the handle came from PowerCreateRequest and is closed once.
        unsafe {
            let h = HANDLE(self.0 as *mut _);
            let _ = PowerClearRequest(h, PowerRequestSystemRequired);
            let _ = CloseHandle(h);
        }
    }
}

#[tauri::command(async)]
pub fn wake_for_tasks(state: AppStateRef) -> bool {
    on(&state.db.lock().unwrap())
}

#[tauri::command(async)]
pub fn set_wake_for_tasks(state: AppStateRef, on: bool) -> Result<bool, String> {
    let conn = state.db.lock().unwrap();
    db::set(&conn, KEY, &on)?;
    db::log_action(&conn, "agent", if on { "Scheduled tasks may wake this PC from sleep" } else { "Scheduled tasks no longer wake this PC" });
    if !on {
        let _ = wake_at(None);
    }
    Ok(on)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Windows takes the wake timer (set, moved, cancelled), and a task can
    /// hold the PC awake.
    #[test]
    fn a_wake_timer_is_set_and_cancelled() {
        assert!(wake_at(Some(db::now_ms() + 3_600_000)));
        assert!(wake_at(Some(db::now_ms() + 7_200_000)));
        assert!(wake_at(None));
        assert!(Awake::new("A test").is_some());
    }
}
