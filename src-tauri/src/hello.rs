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

/// macOS: Touch ID, or the account password where there's none, through
/// LocalAuthentication from JavaScript for Automation (nothing native is
/// linked). The prompt names osascript as the asker.
#[cfg(target_os = "macos")]
mod imp {
    use std::process::Command;

    /// Policy 2: device owner authentication (Touch ID, a watch or the password).
    const CAN: &str = "ObjC.import('LocalAuthentication'); $.LAContext.alloc.init.canEvaluatePolicyError(2, null) ? 'yes' : 'no'";

    fn script(message: &str) -> String {
        let reason = serde_json::to_string(message).unwrap_or_else(|_| "\"Unlock SulcusAI\"".into());
        format!(
            "ObjC.import('LocalAuthentication'); ObjC.import('Foundation'); var done = null;              $.LAContext.alloc.init.evaluatePolicyLocalizedReasonReply(2, {reason}, function (ok, err) {{ done = ok ? 'yes' : 'no'; }});              while (done === null) {{ $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(0.1)); }} done"
        )
    }

    fn ask(js: &str) -> Result<String, String> {
        let out = Command::new("osascript").args(["-l", "JavaScript", "-e", js]).output().map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn available() -> bool {
        ask(CAN).is_ok_and(|s| s == "yes")
    }

    pub fn verify(_hwnd: isize, message: &str) -> Result<bool, String> {
        Ok(ask(&script(message))? == "yes")
    }
}

/// Linux: the desktop's own authentication prompt through polkit (`pkexec`),
/// which asks for the account password, or a fingerprint where fprintd is
/// set up for it. Nothing runs as root: the command is `true`.
#[cfg(target_os = "linux")]
mod imp {
    pub fn available() -> bool {
        let desktop = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
        desktop && crate::engine::on_path("pkexec").is_some() && crate::engine::on_path("true").is_some()
    }

    pub fn verify(_hwnd: isize, _message: &str) -> Result<bool, String> {
        let truth = crate::engine::on_path("true").ok_or("The system's \"true\" program wasn't found.")?;
        let status = std::process::Command::new("pkexec").arg("--disable-internal-agent").arg(truth).status().map_err(|e| e.to_string())?;
        // 126: the prompt was dismissed; 127: not authorized.
        Ok(status.success())
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub fn available() -> bool {
        false
    }
    pub fn verify(_hwnd: isize, _message: &str) -> Result<bool, String> {
        Err("Unlocking with the system's sign-in isn't available on this system.".into())
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
