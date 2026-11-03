// SPDX-License-Identifier: AGPL-3.0-only
//! Reaching the running copy from outside: the Start menu's right-click
//! entries (a jump list) start `sulcusai.exe --pause` and the like, and that
//! second start hands the word to the running copy through a named Windows
//! event, then ends. The running copy waits on the events in one thread
//! that sleeps until one is set.

/// What a second start can ask of the running copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Pause,
    Resume,
    Quit,
}

impl Ask {
    const ALL: [Ask; 3] = [Ask::Pause, Ask::Resume, Ask::Quit];

    pub fn arg(self) -> &'static str {
        match self {
            Ask::Pause => "--pause",
            Ask::Resume => "--resume",
            Ask::Quit => "--quit",
        }
    }

    #[cfg(windows)]
    fn event_name(self) -> String {
        // Tests use names of their own, so they never reach an installed copy.
        let prefix = if cfg!(test) { "Local\\SulcusAI-test." } else { "Local\\SulcusAI." };
        format!("{prefix}{}", &self.arg()[2..])
    }
}

/// The ask on this start's command line, if any.
pub fn asked(args: &[String]) -> Option<Ask> {
    args.iter().skip(1).find_map(|a| Ask::ALL.into_iter().find(|k| k.arg() == a))
}

/// Hands `ask` to a running copy. False when none is running.
#[cfg(windows)]
pub fn forward(ask: Ask) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    // SAFETY: a named event opened, set and closed; no memory is shared.
    unsafe {
        let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(ask.event_name())) else { return false };
        let ok = SetEvent(h).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

#[cfg(not(windows))]
pub fn forward(_ask: Ask) -> bool {
    false
}

/// In the running copy: calls `on` for every ask a second start hands over.
#[cfg(windows)]
pub fn listen(on: impl Fn(Ask) + Send + 'static) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};
    let mut handles: Vec<HANDLE> = Vec::new();
    for ask in Ask::ALL {
        // SAFETY: creates an auto-reset event this process keeps for its life.
        match unsafe { CreateEventW(None, false, false, &HSTRING::from(ask.event_name())) } {
            Ok(h) => handles.push(h),
            Err(e) => {
                crate::applog::line("app", &format!("Start menu entries can't reach SulcusAI: {e}"));
                return;
            }
        }
    }
    let raw: Vec<isize> = handles.iter().map(|h| h.0 as isize).collect();
    std::thread::Builder::new()
        .name("start-menu-asks".into())
        .spawn(move || {
            let handles: Vec<HANDLE> = raw.into_iter().map(|h| HANDLE(h as *mut _)).collect();
            loop {
                // SAFETY: waits on events this process owns and never closes.
                let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                match Ask::ALL.get(i) {
                    Some(ask) => on(*ask),
                    None => return,
                }
            }
        })
        .ok();
}

#[cfg(not(windows))]
pub fn listen(_on: impl Fn(Ask) + Send + 'static) {}

/// The Start menu's right-click entries: Pause completely, Resume and Quit
/// completely, or none (`on` false).
#[cfg(windows)]
pub fn set_jump_list(on: bool, titles: [&str; 3]) -> Result<(), String> {
    use windows::core::{Interface, HSTRING, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSCoerceToCanonicalValue, PROPERTYKEY};
    use windows::Win32::UI::Shell::{DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink};
    // PKEY_Title, which names a jump list entry.
    const TITLE: PROPERTYKEY = PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9), pid: 2 };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = HSTRING::from(exe.as_os_str());
    let titles = titles.map(str::to_string);
    // The shell wants a single-threaded COM apartment: a thread of its own.
    std::thread::spawn(move || -> Result<(), String> {
        // SAFETY: COM objects made, used and released on this thread, with
        // strings that outlive each call.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let r = (|| -> windows::core::Result<()> {
                let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
                // The installer gives the Start menu shortcut this id.
                list.SetAppID(&HSTRING::from("app.sulcusai.desktop"))?;
                if !on {
                    return list.DeleteList(&HSTRING::from("app.sulcusai.desktop"));
                }
                let mut slots = 0u32;
                let _removed: IObjectArray = list.BeginList(&mut slots)?;
                let tasks: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
                for (ask, title) in Ask::ALL.into_iter().zip(titles) {
                    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    link.SetPath(&exe)?;
                    link.SetArguments(&HSTRING::from(ask.arg()))?;
                    link.SetIconLocation(&exe, 0)?;
                    let store: IPropertyStore = link.cast()?;
                    let mut name = PROPVARIANT::from(title.as_str());
                    PSCoerceToCanonicalValue(&TITLE, &mut name)?;
                    store.SetValue(&TITLE, &name)?;
                    store.Commit()?;
                    tasks.AddObject(&link)?;
                }
                list.AddUserTasks(&tasks.cast::<IObjectArray>()?)?;
                list.CommitList()
            })();
            if com {
                CoUninitialize();
            }
            r.map_err(|e| e.message().to_string())
        }
    })
    .join()
    .map_err(|_| "The Start menu entries couldn't be set.".to_string())?
}

#[cfg(not(windows))]
pub fn set_jump_list(_on: bool, _titles: [&str; 3]) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_names_what_it_asks_of_the_running_copy() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(asked(&args(&["sulcusai.exe", "--pause"])), Some(Ask::Pause));
        assert_eq!(asked(&args(&["sulcusai.exe", "--quit"])), Some(Ask::Quit));
        assert_eq!(asked(&args(&["sulcusai.exe", "--resume"])), Some(Ask::Resume));
        assert_eq!(asked(&args(&["sulcusai.exe"])), None);
        // The program's own path is never read as an ask.
        assert_eq!(asked(&args(&["--pause"])), None);
    }

    #[cfg(windows)]
    #[test]
    fn an_ask_reaches_a_listening_copy_and_none_without_one() {
        let (tx, rx) = std::sync::mpsc::channel();
        listen(move |a| {
            let _ = tx.send(a);
        });
        assert!(forward(Ask::Resume));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), Ask::Resume);
    }
}
