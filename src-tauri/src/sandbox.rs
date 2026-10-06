// SPDX-License-Identifier: AGPL-3.0-only
//! Folders the user lets the agent work in. Every path a tool touches is
//! resolved here and refused unless it is inside a granted folder.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    roots: Vec<PathBuf>,
}

fn canonical(p: &Path) -> Option<PathBuf> {
    dunce::canonicalize(p).ok()
}

/// Lowercased with `/` separators, for case-insensitive prefix checks.
fn key(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('\\', "/").to_lowercase();
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let (p, r) = (key(path), key(root));
    p == r || p.starts_with(&format!("{r}/"))
}

/// Folders that must never be granted: drive roots and system locations.
pub fn refuse_reason(path: &Path, app_data: &Path) -> Option<&'static str> {
    let Some(c) = canonical(path) else { return Some("That folder doesn't exist.") };
    if !c.is_dir() {
        return Some("That isn't a folder.");
    }
    if c.parent().is_none() || c.components().count() <= 1 {
        return Some("A whole drive can't be granted. Pick a folder inside it.");
    }
    let env_dirs = ["WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"];
    for var in env_dirs {
        if let Some(dir) = std::env::var_os(var).map(PathBuf::from).and_then(|d| canonical(&d)) {
            if is_inside(&c, &dir) || is_inside(&dir, &c) {
                return Some("System folders can't be granted.");
            }
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from).and_then(|d| canonical(&d)) {
        if key(&c) == key(&home) {
            return Some("Your whole user folder is too broad. Pick a folder inside it, like Documents or a project folder.");
        }
    }
    if let Some(data) = canonical(app_data) {
        if is_inside(&c, &data) || is_inside(&data, &c) {
            return Some("SulcusAI's own data folder can't be granted.");
        }
    }
    None
}

impl Sandbox {
    pub fn new(folders: &[String]) -> Sandbox {
        Sandbox { roots: folders.iter().filter_map(|f| canonical(Path::new(f))).filter(|p| p.is_dir()).collect() }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Resolves a path the model gave (absolute, or relative to the first
    /// granted folder). Refuses anything outside the granted folders,
    /// including `..` tricks and links that point elsewhere.
    pub fn resolve(&self, input: &str) -> Result<PathBuf, String> {
        let first = self.roots.first().ok_or("No folders are shared with SulcusAI yet. Add one with the 📁 button.")?;
        let trimmed = input.trim();
        // "/proj/x" has no drive letter, so on Windows it means "proj/x".
        let raw = if Path::new(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            PathBuf::from(trimmed.trim_start_matches(['/', '\\']))
        };
        let raw = raw.as_path();
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            // Paths shown to the model start with the granted folder's name
            // ("proj/src/main.rs"); map that name back to the folder.
            let mut parts = raw.components();
            let head = parts.next().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
            let named = self.roots.iter().find(|r| {
                r.file_name().map(|n| n.to_string_lossy().to_lowercase()) == head
            });
            match named {
                Some(root) => root.join(parts.as_path()),
                None => first.join(raw),
            }
        };

        // Resolve `..` lexically first, then follow links on the part that exists.
        let mut lexical = PathBuf::new();
        for c in joined.components() {
            match c {
                Component::ParentDir => {
                    lexical.pop();
                }
                Component::CurDir => {}
                other => lexical.push(other.as_os_str()),
            }
        }
        let mut existing = lexical.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !existing.exists() {
            match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
                (Some(name), Some(parent)) => {
                    tail.push(name);
                    existing = parent;
                }
                _ => return Err(format!("{input} isn't in a folder you've shared.")),
            }
        }
        let mut resolved = canonical(&existing).ok_or_else(|| format!("Couldn't open {input}."))?;
        for name in tail.into_iter().rev() {
            resolved.push(name);
        }
        if self.roots.iter().any(|r| is_inside(&resolved, r)) {
            Ok(resolved)
        } else {
            Err(format!("{input} is outside the folders you've shared with SulcusAI."))
        }
    }

    /// The path as the model should see it: relative to its granted folder when possible.
    pub fn display(&self, p: &Path) -> String {
        for r in &self.roots {
            if let Ok(rel) = p.strip_prefix(r) {
                let name = r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let rel = rel.to_string_lossy().replace('\\', "/");
                return if rel.is_empty() { name } else { format!("{name}/{rel}") };
            }
        }
        p.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sulcusai-sb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("proj/src")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        std::fs::write(d.join("proj/src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(d.join("other/secret.txt"), "x").unwrap();
        d
    }

    #[test]
    fn paths_inside_resolve_and_outside_are_refused() {
        let d = temp();
        let sb = Sandbox::new(&[d.join("proj").display().to_string()]);
        assert!(sb.resolve("src/main.rs").unwrap().ends_with("main.rs"));
        assert!(sb.resolve(&d.join("proj/src/main.rs").display().to_string()).is_ok());
        assert!(sb.resolve("src/new_file.rs").is_ok(), "new files inside are fine");
        assert!(sb.resolve("src/new_dir/deep/file.rs").is_ok());
        assert!(sb.resolve("../other/secret.txt").is_err(), "parent escape");
        assert!(sb.resolve("src/../../other/secret.txt").is_err());
        assert!(sb.resolve(&d.join("other/secret.txt").display().to_string()).is_err());
        // A sibling whose name starts with the root's name is not inside it.
        std::fs::create_dir_all(d.join("proj-evil")).unwrap();
        assert!(sb.resolve(&d.join("proj-evil").display().to_string()).is_err());
    }

    #[test]
    fn case_differences_still_match_on_windows() {
        let d = temp();
        let sb = Sandbox::new(&[d.join("proj").display().to_string()]);
        let upper = d.join("PROJ").join("SRC").join("MAIN.RS").display().to_string();
        if cfg!(windows) {
            assert!(sb.resolve(&upper).is_ok());
        }
    }

    #[test]
    fn no_folders_means_nothing_resolves() {
        assert!(Sandbox::new(&[]).resolve("anything").is_err());
    }

    #[test]
    fn dangerous_folders_cannot_be_granted() {
        let d = temp();
        let app_data = d.join("appdata");
        std::fs::create_dir_all(&app_data).unwrap();
        assert!(refuse_reason(Path::new("C:\\"), &app_data).is_some());
        if let Some(win) = std::env::var_os("WINDIR") {
            assert!(refuse_reason(Path::new(&win), &app_data).is_some());
        }
        assert!(refuse_reason(&app_data, &app_data).is_some());
        assert!(refuse_reason(&d.join("proj"), &app_data).is_none());
        assert!(refuse_reason(&d.join("missing"), &app_data).is_some());
    }

    #[test]
    fn display_is_relative_to_the_granted_folder() {
        let d = temp();
        let sb = Sandbox::new(&[d.join("proj").display().to_string()]);
        let p = sb.resolve("src/main.rs").unwrap();
        assert_eq!(sb.display(&p), "proj/src/main.rs");
        // What the model is shown resolves back to the same file.
        assert_eq!(sb.resolve("proj/src/main.rs").unwrap(), p);
        assert_eq!(sb.resolve("/proj/src/main.rs").unwrap(), p);
        assert_eq!(sb.resolve("proj").unwrap(), sb.roots()[0]);
    }

    #[test]
    fn several_folders_are_told_apart_by_name() {
        let d = temp();
        let sb = Sandbox::new(&[d.join("proj").display().to_string(), d.join("other").display().to_string()]);
        assert!(sb.resolve("other/secret.txt").unwrap().ends_with("secret.txt"));
        assert!(sb.resolve("proj/src/main.rs").is_ok());
    }
}
