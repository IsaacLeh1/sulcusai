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
    }
    tauri_build::try_build(attrs).expect("tauri build");
}
