// SPDX-License-Identifier: AGPL-3.0-only
//! Keeping the app still while nothing is asked of it.
//!
//! Background loops wait on events rather than ticking: a loop that only
//! looks after loaded things (models, speech servers) sleeps in `stirred`
//! until something loads, and every loop's sleep is a `nap`, which also holds
//! it while the app is paused completely (pause.rs).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::watch;

static PAUSED: AtomicBool = AtomicBool::new(false);
/// Bumped on every pause or resume, so waiters see the change.
static PAUSE: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);
/// Bumped whenever something heavy starts (a model, a speech server, a
/// picture job), so the loops that watch loaded things wake up.
static STIR: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

pub(crate) fn set_paused(on: bool) {
    PAUSED.store(on, Ordering::SeqCst);
    PAUSE.send_replace(on);
}

/// Returns at once unless the app is paused, else when it resumes.
pub async fn awake() {
    let mut rx = PAUSE.subscribe();
    while *rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// A loop's sleep: `d`, and then for as long as the app stays paused.
pub async fn nap(d: Duration) {
    tokio::time::sleep(d).await;
    awake().await;
}

/// Bumped when a setting, a feature, a task or the lock changes, so loops
/// waiting for something to be turned on look again.
static CHANGE: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

pub fn changed() {
    CHANGE.send_modify(|n| *n = n.wrapping_add(1));
}

pub fn change_watcher() -> watch::Receiver<u64> {
    CHANGE.subscribe()
}

/// Sleeps until something changes or `longest` passes (a backstop for a
/// change made somewhere that doesn't call `changed`), then until awake.
pub async fn until_changed(rx: &mut watch::Receiver<u64>, longest: Duration) {
    let _ = tokio::time::timeout(longest, rx.changed()).await;
    awake().await;
}

/// How long a loop with nothing to do waits for a change before looking
/// anyway.
pub const BACKSTOP: Duration = Duration::from_secs(3600);

/// Something that the watching loops look after has started.
pub fn stir() {
    STIR.send_modify(|n| *n = n.wrapping_add(1));
}

/// A loop's own view of `stir`: take it before checking whether anything is
/// loaded, so a load in between isn't missed.
pub fn watcher() -> watch::Receiver<u64> {
    STIR.subscribe()
}

/// Waits for the next `stir` this receiver hasn't seen, then for the app to
/// be awake.
pub async fn stirred(rx: &mut watch::Receiver<u64>) {
    let _ = rx.changed().await;
    awake().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_nap_holds_while_paused_and_ends_on_resume() {
        set_paused(true);
        let napping = tokio::spawn(nap(Duration::from_millis(5)));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!napping.is_finished(), "a nap ended while the app was paused");
        set_paused(false);
        tokio::time::timeout(Duration::from_secs(2), napping).await.expect("the nap ended on resume").unwrap();
    }

    #[tokio::test]
    async fn a_stir_before_waiting_is_not_missed() {
        let mut rx = watcher();
        rx.borrow_and_update();
        stir();
        tokio::time::timeout(Duration::from_secs(2), stirred(&mut rx)).await.expect("the stir that came first was seen");
    }
}
