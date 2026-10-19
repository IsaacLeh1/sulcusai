// SPDX-License-Identifier: AGPL-3.0-only
//! Keeping the PC safe from the assistant's commands: commands that touch
//! disks, booting, backups or Windows' defences are never run, nor ones that
//! delete or overwrite things outside the shared folders; and none run while
//! the disk is nearly full (running ones are stopped when it gets there).

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::sandbox::Sandbox;

const GB: u64 = 1024 * 1024 * 1024;
/// Free space a command needs to start.
pub const START_FREE: u64 = 4 * GB;
/// Below this, running commands are stopped, so Windows and other apps
/// (Steam downloads, updates) always have room to finish writing.
pub const STOP_FREE: u64 = 2 * GB;

/// Commands never run, whatever the mode: with the reason the model is told.
static NEVER: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let disk = "It works on disks, partitions or the file system itself";
    let boot = "It changes how Windows starts or recovers";
    let backup = "It deletes or changes backups and restore points";
    let defence = "It turns off or weakens Windows' protection";
    let power = "It shuts down or restarts the PC";
    [
        (r"\bdiskpart\b", disk),
        (r"\bformat(\.com)?\s+['\x22]?[a-z]:", disk),
        (r"\b(format-volume|clear-disk|initialize-disk|new-partition|remove-partition|resize-partition|set-partition|set-disk|remove-physicaldisk|reset-physicaldisk|repair-volume|new-volume|remove-virtualdisk|dismount-diskimage)\b", disk),
        (r"\bchkdsk\b", disk),
        (r"\bfsutil\b", disk),
        (r"\bmountvol\b", disk),
        (r"\bcipher(\.exe)?\s+/w", disk),
        (r"\bsdelete(64)?\b", disk),
        (r"\\\\[.?]\\(physicaldrive|harddisk|volume\{|[a-z]:)", disk),
        (r"\b(manage-bde|disable-bitlocker|enable-bitlocker|suspend-bitlocker|clear-bitlockerautounlock)\b", disk),
        (r"\b(bcdedit|bcdboot|bootrec|bootsect|reagentc)\b", boot),
        (r"\bdism(\.exe)?\b", boot),
        (r"\b(vssadmin|wbadmin)\b|\bshadowcopy\b|\b(disable-computerrestore|remove-computerrestorepoint)\b", backup),
        (r"\bset-mppreference\b|\badd-mppreference\b[^\n]*-exclusion", defence),
        (r"\bnetsh\b[^\n]*\bstate\s+off\b|\bset-netfirewallprofile\b[^\n]*-enabled\s+\$?(false|0)", defence),
        (r"\bset-executionpolicy\b[^\n]*-scope\s+localmachine", defence),
        (r"\breg(\.exe)?\s+(delete|add|import|load|unload|restore|copy)\s+['\x22]?(hklm|hkey_local_machine)", defence),
        (r"\b(remove-item|set-itemproperty|new-itemproperty|remove-itemproperty|rename-itemproperty|new-item|set-item)\b[^\n]*\bhklm:", defence),
        (r"\btakeown\b", defence),
        (r"\b(shutdown(\.exe)?\s+[/-][srpghf]|stop-computer|restart-computer)\b", power),
    ]
    .into_iter()
    .map(|(p, why)| (Regex::new(&format!("(?i){p}")).expect("a valid pattern"), why))
    .collect()
});

/// Words that delete, move or change permissions: every path such a command
/// names must be inside the shared folders.
static DESTROYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[\s;|&(])(remove-item|ri|rm|rmdir|rd|del|erase|clear-content|clear-item|move-item|mv|move|robocopy|rimraf|icacls|attrib)(\.exe)?([\s;|&)]|$)")
        .expect("a valid pattern")
});

/// Words that write a file: only what they write to must be inside the
/// shared folders (what they read may be anywhere: listing the home folder
/// into a report in a shared folder is fine).
static WRITES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:^|[\s;|&(])(?:set-content|out-file|add-content|tee-object|tee)(?:\.exe)?\s+(?:-(?:filepath|path|literalpath)\s+)?['"]?([^\s'";|&)<>]+)"#).expect("a valid pattern")
});

/// Output sent into any file, relative ones too: `> out.txt`.
static ANY_REDIRECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[^2&]>>?\s*['"]?([^\s'";|&)<>]+)"#).expect("a valid pattern"));

/// Other commands that throw work away: hard resets, force pushes, killing programs.
static THROWS_AWAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bgit\s+(reset\s+--hard|clean\s+-[a-z]*f|push\s+[^\n]*(-f\b|--force)|checkout\s+--\s|restore\s)|\b(stop-process|taskkill|kill|pkill|killall)\b|\b(npm|pnpm|yarn|pip|winget|choco)\s+(uninstall|remove)\b")
        .expect("a valid pattern")
});

/// A path in a command, from where it starts: absolute, from the home
/// folder or a folder Windows names by a variable, or going up with `..`.
const PATH: &str = r#"[a-z]:(?:[\\/][^\s'";|&)<>]*)?|\\\\[^\s'";|&)<>]+|(?:~|\$home|\$env:[a-z_()0-9]+|%[a-z_()0-9]+%)(?:[\\/][^\s'";|&)<>]*)?|\.\.(?:[\\/][^\s'";|&)<>]*)?"#;

static PATHS: LazyLock<Regex> = LazyLock::new(|| {
    let unix = if cfg!(windows) { "" } else { r#"|/[^\s'";|&)<>]*"# };
    Regex::new(&format!(r#"(?i)(?:^|[\s'"=(,])({PATH}{unix})"#)).expect("a valid pattern")
});

/// Why the assistant may not run this command, if it may not.
pub fn refuse(command: &str, dir: &Path, sandbox: &Sandbox) -> Option<String> {
    for (pattern, why) in NEVER.iter() {
        if pattern.is_match(command) {
            return Some(format!(
                "SulcusAI never runs this command. {why}, which could damage Windows or the user's files. \
                 Don't try another way: tell the user what you'd run and why, so they can decide and do it themselves."
            ));
        }
    }
    let destroys = DESTROYS.is_match(command);
    let writes = WRITES.is_match(command) || ANY_REDIRECT.is_match(command);
    // Running in a folder that isn't shared (a program found in the home
    // folder): a command that deletes or writes there would change it.
    if (destroys || writes) && !sandbox.holds(dir) {
        return Some(format!(
            "This command changes files, and it would run in {}, which isn't a shared folder, so SulcusAI can read there but not change anything. \
             Run it in a shared folder, or call request_folder to share that folder first.",
            dir.display()
        ));
    }
    // Every path, when the command deletes or moves; else only what it writes to.
    let named: Vec<String> = if destroys {
        PATHS.captures_iter(command).filter_map(|c| c.get(1)).map(|m| m.as_str().to_string()).collect()
    } else {
        WRITES.captures_iter(command).chain(ANY_REDIRECT.captures_iter(command)).filter_map(|c| c.get(1)).map(|m| m.as_str().to_string()).collect()
    };
    let outside: Vec<String> = named
        .into_iter()
        .map(|p| p.trim_end_matches([',']).to_string())
        // Output thrown away isn't written anywhere.
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("$null") && !p.eq_ignore_ascii_case("nul") && p != "/dev/null")
        .filter(|p| !sandbox.holds(&expand(p, dir)))
        .collect();
    (!outside.is_empty()).then(|| {
        format!(
            "This command could delete or overwrite things outside the shared folders ({}). Commands may only change \
             things inside the shared folders. Use paths inside them, or call request_folder to share the folder first.",
            outside.join(", ")
        )
    })
}

/// A command the user should see before it runs: it deletes, moves or
/// changes permissions on files, or throws work away (a hard reset, a force
/// push, stopping programs, uninstalling). Everything else the guard lets
/// through runs without asking.
pub fn destructive(command: &str) -> bool {
    DESTROYS.is_match(command) || THROWS_AWAY.is_match(command)
}

/// A path from a command, as the folder it names: variables filled in and
/// relative paths taken from `dir`.
fn expand(p: &str, dir: &Path) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    let (head, rest) = match p.find(['\\', '/']) {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (p, ""),
    };
    let lower = head.to_lowercase();
    let base = if lower == "~" || lower == "$home" {
        Some(home)
    } else if let Some(name) = lower.strip_prefix("$env:") {
        Some(std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("C:\\{name}"))))
    } else if lower.len() > 2 && lower.starts_with('%') && lower.ends_with('%') {
        Some(std::env::var_os(&lower[1..lower.len() - 1]).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\")))
    } else {
        None
    };
    let full = match base {
        Some(b) => b.join(rest),
        None if Path::new(p).is_absolute() || p.starts_with("\\\\") || (p.len() == 2 && p.ends_with(':')) => PathBuf::from(if p.len() == 2 { format!("{p}\\") } else { p.to_string() }),
        None => dir.join(p),
    };
    tidy(&full)
}

/// `a/b/../c` as `a/c`, without touching the disk.
fn tidy(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The least free space on the drives a command uses: its folder's and the
/// system drive (where temp files and package caches go).
pub fn least_free(dir: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let free_on = |p: &Path| {
        disks.list().iter().filter(|d| p.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len()).map(|d| d.available_space())
    };
    let system = std::env::var_os("SystemDrive").map(|d| PathBuf::from(format!("{}\\", d.to_string_lossy())));
    [Some(dir.to_path_buf()), system].into_iter().flatten().filter_map(|p| free_on(&p)).min()
}

/// Why no command may start now (the disk is nearly full), if so.
pub fn disk_too_full(dir: &Path) -> Option<String> {
    let free = least_free(dir)?;
    (free < START_FREE).then(|| {
        format!(
            "The disk is nearly full ({:.1} GB free), so SulcusAI won't run commands until there's at least {} GB free. \
             Tell the user, so they can make room (Settings → System → Storage in Windows).",
            free as f64 / GB as f64,
            START_FREE / GB
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> (Sandbox, PathBuf) {
        let d = std::env::temp_dir().join(format!("sulcusai-guard-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(d.join("site")).unwrap();
        let d = dunce::canonicalize(d).unwrap();
        (Sandbox::new(&[d.display().to_string()]), d)
    }

    #[test]
    fn disk_and_system_commands_never_run() {
        let (sb, d) = shared();
        for c in [
            "diskpart /s script.txt",
            "format D: /q",
            "Format-Volume -DriveLetter D",
            "chkdsk C: /f",
            "fsutil dirty set C:",
            "cipher /w:C:\\",
            "Clear-Disk -Number 1 -RemoveData",
            "bcdedit /set {current} safeboot minimal",
            "vssadmin delete shadows /all /quiet",
            "Set-MpPreference -DisableRealtimeMonitoring $true",
            "reg delete HKLM\\Software\\Valve /f",
            "Remove-Item -Path HKLM:\\Software\\Thing -Recurse",
            "shutdown /s /t 0",
            "Get-Content \\\\.\\PhysicalDrive0",
        ] {
            assert!(refuse(c, &d, &sb).is_some_and(|w| w.contains("never runs")), "{c}");
        }
    }

    #[test]
    fn deleting_outside_the_shared_folders_is_refused() {
        let (sb, d) = shared();
        let inside = d.join("site").join("old");
        // Windows' paths there; on macOS and Linux, "C:\..." is just a name.
        let outside: &[&str] = if cfg!(windows) {
            &[
                "Remove-Item -Recurse -Force C:\\Program Files (x86)\\Steam\\steamapps",
                "rm -r -fo C:\\",
                "del /s /q C:",
                "Remove-Item $env:LOCALAPPDATA\\Temp\\* -Recurse",
                "Remove-Item ~\\Documents -Recurse",
                "rd /s /q ..\\..",
                "Get-Process | Out-File C:\\Windows\\log.txt",
                "echo hi > C:\\Users\\Public\\x.txt",
            ]
        } else {
            &["rm -rf /usr/local/lib", "rm -r ~/Documents", "rm -rf ../..", "mv dist /etc/nginx/html", "echo hi > /etc/hosts"]
        };
        for c in outside {
            assert!(refuse(c, &d, &sb).is_some_and(|w| w.contains("outside the shared folders")), "{c}");
        }
        // Inside them, and everyday build commands, are fine.
        for c in [
            format!("Remove-Item -Recurse -Force {}", inside.display()),
            "Remove-Item -Recurse -Force node_modules, dist".to_string(),
            "rm -rf site/build".to_string(),
            "npm install; npm run build".to_string(),
            "& \"C:\\Program Files\\nodejs\\node.exe\" server.js".to_string(),
            "python -m http.server 8000".to_string(),
            "node build.js > out.txt".to_string(),
            "git status".to_string(),
        ] {
            assert_eq!(refuse(&c, &d, &sb), None, "{c}");
        }
    }

    /// Writing a report into the shared folder from what's outside it is
    /// fine; writing outside, or running a changing command in a folder
    /// that isn't shared, isn't.
    #[test]
    fn only_what_a_command_writes_must_be_shared() {
        let (sb, d) = shared();
        let ok: &[&str] = if cfg!(windows) {
            &["Get-ChildItem $env:USERPROFILE -Recurse -Filter *.txt | Out-File report.txt", "Get-Content C:\\Windows\\win.ini > copy.txt", "dir C:\\Users > nul", "Start-Process notepad.exe"]
        } else {
            &["ls -R ~ > report.txt", "cat /etc/hosts > hosts.txt", "ls / > /dev/null"]
        };
        for c in ok {
            assert_eq!(refuse(c, &d, &sb), None, "{c}");
        }
        let written_outside = if cfg!(windows) { "Get-ChildItem | Out-File -FilePath C:\\Windows\\report.txt" } else { "ls | tee /etc/report.txt" };
        assert!(refuse(written_outside, &d, &sb).is_some_and(|w| w.contains("outside the shared folders")));
        // Running in a folder that isn't shared: reading is fine, changing isn't.
        let elsewhere = std::env::temp_dir().join(format!("sulcusai-guard-out-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&elsewhere).unwrap();
        let elsewhere = dunce::canonicalize(elsewhere).unwrap();
        assert_eq!(refuse("hello.bat", &elsewhere, &sb), None, "running a program found there");
        assert!(refuse("echo hi > out.txt", &elsewhere, &sb).is_some_and(|w| w.contains("isn't a shared folder")));
        assert!(refuse("del old.txt", &elsewhere, &sb).is_some());
    }

    /// What asks the user first in Auto mode: deleting, moving, and throwing work away.
    #[test]
    fn destructive_commands_are_told_apart() {
        for c in ["Remove-Item build -Recurse", "rm -rf dist", "git reset --hard HEAD~1", "git push --force", "taskkill /im node.exe", "npm uninstall react", "move a.txt b.txt"] {
            assert!(destructive(c), "{c}");
        }
        for c in ["npm install", "npm run build", "python script.py", "Start-Process notepad.exe", "git status", "Get-ChildItem -Recurse", "node build.js > out.txt", ".\\hello.bat"] {
            assert!(!destructive(c), "{c}");
        }
    }

    #[test]
    fn the_disk_check_reads_this_pc() {
        let (_, d) = shared();
        assert!(least_free(&d).is_some_and(|f| f > 0));
    }
}
