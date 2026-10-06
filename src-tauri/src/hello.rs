// SPDX-License-Identifier: AGPL-3.0-only
//! Windows Hello (face, fingerprint or Windows PIN) as a way to unlock.
//! Both calls block, so run them on a blocking thread.

#[cfg(windows)]
mod imp {
    use windows::core::{factory, HSTRING};
    use windows::Foundation::IAsyncOperation;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::{IUserConsentVerifierInterop, RoInitialize, RO_INIT_MULTITHREADED};

    fn init() {
        // SAFETY: initializes WinRT on this worker thread; "already initialized" is fine.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    }

    pub fn available() -> bool {
        init();
        UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|op| op.get())
            .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
    }

    /// Shows the Windows Hello prompt over the app window. True if verified.
    pub fn verify(hwnd: isize, message: &str) -> Result<bool, String> {
        init();
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>().map_err(|e| e.to_string())?;
        // SAFETY: hwnd is the live main window handle.
        let op: IAsyncOperation<UserConsentVerificationResult> =
            unsafe { interop.RequestVerificationForWindowAsync(HWND(hwnd as _), &HSTRING::from(message)) }
                .map_err(|e| e.to_string())?;
        let result = op.get().map_err(|e| e.to_string())?;
        Ok(result == UserConsentVerificationResult::Verified)
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn available() -> bool {
        false
    }
    pub fn verify(_hwnd: isize, _message: &str) -> Result<bool, String> {
        Err("Windows Hello is only available on Windows.".into())
    }
}

pub use imp::{available, verify};

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn availability_check_runs_without_prompting() {
        // Only asks whether Hello is set up; never shows a prompt.
        let _ = super::available();
    }
}
