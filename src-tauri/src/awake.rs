// SPDX-License-Identifier: AGPL-3.0-only
//! Keeps the PC from going to sleep while long work runs. The screen can
//! still turn off, and closing a laptop's lid still sleeps it.
//!
//! Windows ties the request to the thread that makes it, and async tasks
//! move between threads, so a thread of its own holds it until the guard
//! is dropped.

#[cfg(windows)]
pub struct KeepAwake(#[allow(dead_code)] std::sync::mpsc::Sender<()>);

#[cfg(windows)]
impl KeepAwake {
    pub fn new() -> Self {
        use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            // SAFETY: only sets this thread's power request flags.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            // Returns when the guard's sender is dropped.
            let _ = rx.recv();
            // SAFETY: as above, clearing them.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
        KeepAwake(tx)
    }
}

#[cfg(not(windows))]
pub struct KeepAwake;

#[cfg(not(windows))]
impl KeepAwake {
    pub fn new() -> Self {
        KeepAwake
    }
}
