// SPDX-License-Identifier: AGPL-3.0-only
fn main() {
    // The app manifest (Common Controls v6) is linked into every binary,
    // tests included: window code (the browser) can't load without it.
    let attrs = tauri_build::Attributes::new().windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    #[cfg(windows)]
    {
        let manifest = std::env::current_dir().unwrap().join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // 8 MB for the main thread (Windows gives 1 MB): commands start on
        // it, and sending a message in 0.9.26 overflowed it, closing the app.
        // Only what's used is committed.
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
    tauri_build::try_build(attrs).expect("tauri build");
}
