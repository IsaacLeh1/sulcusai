// SPDX-License-Identifier: AGPL-3.0-only
//! What Windows says about power and the screen, heard as it happens rather
//! than checked on a timer: plugging in and out, waking from sleep, the clock
//! changing, and an app (a game, a film) going full screen. One thread with
//! an invisible window waits for these, using nothing in between.
//!
//! Also Windows' efficiency mode (EcoQoS) for SulcusAI's processes, used
//! while paused and while out of the way, and handing memory back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Plugged in or unplugged.
    Power,
    /// Back from sleep, or the clock changed: anything waiting for a time
    /// should look again.
    Woke,
    /// An app went full screen (true) or left it (false).
    FullScreen(bool),
}

static FULL_SCREEN: AtomicBool = AtomicBool::new(false);
static ON_EVENT: OnceLock<Box<dyn Fn(Event) + Send + Sync>> = OnceLock::new();

/// Whether an app other than SulcusAI fills the screen now.
pub fn full_screen() -> bool {
    FULL_SCREEN.load(Ordering::Relaxed)
}

fn tell(e: Event) {
    if let Some(f) = ON_EVENT.get() {
        f(e);
    }
}

/// Starts listening, once; `on` hears every event.
pub fn listen(on: impl Fn(Event) + Send + Sync + 'static) {
    if ON_EVENT.set(Box::new(on)).is_ok() {
        imp::start();
    }
}

/// Watches for full-screen apps (only while the automatic switch for them
/// is on: otherwise switching windows costs nothing here).
pub fn watch_screen(on: bool) {
    imp::watch_screen(on);
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const PBT_APMPOWERSTATUSCHANGE: usize = 0x000A;
    const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
    const PBT_APMRESUMESUSPEND: usize = 0x0007;
    const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
    const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
    const WINEVENT_OUTOFCONTEXT: u32 = 0;
    const WINEVENT_SKIPOWNPROCESS: u32 = 2;
    /// Posted to the invisible window: hooks on (wparam 1) or off (0).
    const WATCH: u32 = WM_APP + 1;

    static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
    static WANT_WATCH: AtomicBool = AtomicBool::new(false);
    thread_local! {
        static HOOKS: std::cell::RefCell<Vec<HWINEVENTHOOK>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub fn watch_screen(on: bool) {
        WANT_WATCH.store(on, Ordering::Relaxed);
        let h = WINDOW.load(Ordering::Relaxed);
        if h != 0 {
            // SAFETY: posts to this app's own invisible window.
            unsafe {
                let _ = PostMessageW(HWND(h as *mut _), WATCH, WPARAM(on as usize), LPARAM(0));
            }
        }
    }

    /// On the window's thread: the hooks that hear windows coming forward.
    unsafe fn set_hooks(on: bool) {
        HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            for h in hooks.drain(..) {
                let _ = UnhookWinEvent(h);
            }
            if on {
                for e in [EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MINIMIZEEND] {
                    hooks.push(SetWinEventHook(e, e, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS));
                }
            }
        });
        if on {
            look();
        } else if FULL_SCREEN.swap(false, Ordering::Relaxed) {
            tell(Event::FullScreen(false));
        }
    }

    pub fn start() {
        std::thread::Builder::new()
            .name("power-events".into())
            .spawn(|| {
                // SAFETY: an invisible window and a hook made and served on
                // this thread, which lives as long as the app.
                unsafe {
                    let Ok(module) = GetModuleHandleW(None) else { return };
                    let class = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: module.into(), lpszClassName: w!("SulcusAIPowerEvents"), ..Default::default() };
                    RegisterClassW(&class);
                    // Top-level (not message-only): power broadcasts reach only those.
                    let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("SulcusAIPowerEvents"), w!(""), WS_OVERLAPPED, 0, 0, 0, 0, None, None, module, None) else { return };
                    WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
                    if WANT_WATCH.load(Ordering::Relaxed) {
                        set_hooks(true);
                    }
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .ok();
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_POWERBROADCAST => match wparam.0 {
                PBT_APMPOWERSTATUSCHANGE => tell(Event::Power),
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => tell(Event::Woke),
                _ => {}
            },
            WM_TIMECHANGE => tell(Event::Woke),
            WATCH => set_hooks(wparam.0 != 0),
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe extern "system" fn on_win_event(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
        look();
    }

    pub fn look() {
        let now = full_screen_now();
        if FULL_SCREEN.swap(now, Ordering::Relaxed) != now {
            tell(Event::FullScreen(now));
        }
    }

    fn full_screen_now() -> bool {
        // SAFETY: read-only queries about windows; the rects are ours.
        unsafe {
            if let Ok(s) = SHQueryUserNotificationState() {
                if s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_BUSY || s == QUNS_PRESENTATION_MODE {
                    return true;
                }
            }
            let fg = GetForegroundWindow();
            if fg.0.is_null() || fg == GetShellWindow() || fg == GetDesktopWindow() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            if pid == std::process::id() {
                return false;
            }
            let mut name = [0u16; 32];
            let n = GetClassNameW(fg, &mut name).max(0) as usize;
            let class = String::from_utf16_lossy(&name[..n]);
            if class == "WorkerW" || class == "Progman" {
                return false;
            }
            // A window with a title bar is an ordinary (maybe maximized) one,
            // even when a hidden taskbar lets it cover the whole screen.
            if GetWindowLongW(fg, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 {
                return false;
            }
            let mut r = RECT::default();
            if GetWindowRect(fg, &mut r).is_err() {
                return false;
            }
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
                return false;
            }
            covers(r, mi.rcMonitor)
        }
    }

    pub(super) fn covers(w: RECT, m: RECT) -> bool {
        w.left <= m.left && w.top <= m.top && w.right >= m.right && w.bottom >= m.bottom
    }

    /// Efficiency mode on (or off) for one process.
    pub fn eco_pid(pid: u32, on: bool) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::*;
        // SAFETY: a handle opened, used for two settings calls and closed.
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) else { return };
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
            };
            let _ = SetProcessInformation(h, ProcessPowerThrottling, &state as *const _ as *const _, std::mem::size_of_val(&state) as u32);
            let _ = SetPriorityClass(h, if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
            let _ = CloseHandle(h);
        }
    }

    /// Hands this process's unused memory back to Windows.
    pub fn trim() {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        // SAFETY: (-1, -1) asks Windows to trim this process's working set.
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}
    pub fn watch_screen(_on: bool) {}
    pub fn eco_pid(_pid: u32, _on: bool) {}
    pub fn trim() {}
}

/// Efficiency mode and idle priority for SulcusAI and everything it started
/// (its window's WebView2 processes, any engine), or back to normal.
pub fn eco(on: bool) {
    for pid in crate::perf::our_pids() {
        imp::eco_pid(pid, on);
    }
}

pub fn trim() {
    imp::trim();
}

#[cfg(all(test, windows))]
mod tests {
    use windows::Win32::Foundation::RECT;

    #[test]
    fn a_window_counts_as_full_screen_only_when_it_covers_its_monitor() {
        let monitor = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert!(super::imp::covers(monitor, monitor));
        assert!(super::imp::covers(RECT { left: -8, top: -8, right: 1928, bottom: 1088 }, monitor), "a maximized-style overhang still covers it");
        assert!(!super::imp::covers(RECT { left: 0, top: 0, right: 1920, bottom: 1040 }, monitor), "a maximized window leaves the taskbar");
        let second = RECT { left: 1920, top: 0, right: 4480, bottom: 1440 };
        assert!(super::imp::covers(second, second));
        assert!(!super::imp::covers(monitor, second));
    }
}
