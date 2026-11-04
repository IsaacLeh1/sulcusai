// SPDX-License-Identifier: AGPL-3.0-only
//! The developer's shared Cloudflare relay (phone link and People pipe) is
//! closing: people move to their own relay, a friend's, or the same Wi-Fi.
//! From `CLOSES` on, the app stops using it; until a date is set it works
//! as before and only the notice shows.

/// Unix seconds when the shared relay closes, once decided.
pub const CLOSES: Option<i64> = None;

pub fn open_at(now: i64) -> bool {
    CLOSES.is_none_or(|c| now < c)
}

pub fn open() -> bool {
    open_at(chrono::Utc::now().timestamp())
}

/// For the notice: the closing date as Unix seconds, if set.
#[tauri::command]
pub fn shared_relay_closes() -> Option<i64> {
    CLOSES
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_until_the_date() {
        assert!(super::open_at(0));
        if let Some(c) = super::CLOSES {
            assert!(super::open_at(c - 1) && !super::open_at(c));
        }
    }
}
