// SPDX-License-Identifier: AGPL-3.0-only
//! Runs an engine program for one job: no window, optionally at low
//! priority, inside the app's job object (so it dies with the app), with
//! its output read line by line for progress and saved to a log.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::download::CANCELLED;
use crate::engine::JobRef;

pub struct Run<'a> {
    pub exe: &'a Path,
    pub args: Vec<OsString>,
    pub cwd: &'a Path,
    pub log: &'a Path,
    pub low_priority: bool,
    pub job: Option<&'a JobRef>,
}

/// Splits output on newlines and on the carriage returns progress bars use.
fn split_lines(buf: &mut Vec<u8>, out: &mut Vec<String>) {
    while let Some(i) = buf.iter().position(|&b| b == b'\n' || b == b'\r') {
        let line: Vec<u8> = buf.drain(..=i).collect();
        let text = String::from_utf8_lossy(&line[..line.len() - 1]).trim_end().to_string();
        // Drop terminal escape codes such as "\x1b[K".
        let text = text.replace("\u{1b}[K", "");
        if !text.trim().is_empty() {
            out.push(text);
        }
    }
}

fn is_error_line(l: &str) -> bool {
    let low = l.to_lowercase();
    l.contains("[E]") || low.contains("error") || low.contains("fatal") || low.contains("failed") || low.contains("out of memory")
}

pub async fn run(r: Run<'_>, cancel: &AtomicBool, mut on_line: impl FnMut(&str)) -> Result<(), String> {
    let mut cmd = Command::new(r.exe);
    cmd.args(&r.args).current_dir(r.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | if r.low_priority { BELOW_NORMAL_PRIORITY_CLASS } else { 0 });
    }
    crate::engine::tie_to_app(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start {}: {e}", r.exe.file_name().unwrap_or_default().to_string_lossy()))?;
    #[cfg(windows)]
    if let (Some(job), Some(pid)) = (r.job, child.id()) {
        job.assign(pid).ok();
    }
    #[cfg(not(windows))]
    let _ = r.job;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut pipe = pipe;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match pipe.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        let mut lines = Vec::new();
                        split_lines(&mut buf, &mut lines);
                        for l in lines {
                            if tx.send(l).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            if !buf.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&buf).trim().to_string());
            }
        });
    }
    drop(tx);

    let mut log = std::fs::File::create(r.log).ok();
    let mut errors: Vec<String> = Vec::new();
    let mut last: Vec<String> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            return Err(CANCELLED.into());
        }
        tokio::select! {
            line = rx.recv() => match line {
                Some(l) => {
                    if let Some(f) = log.as_mut() {
                        let _ = writeln!(f, "{l}");
                    }
                    if is_error_line(&l) {
                        errors.push(l.clone());
                    }
                    last.push(l.clone());
                    if last.len() > 6 {
                        last.remove(0);
                    }
                    on_line(&l);
                }
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    if !status.success() {
        let detail = if errors.is_empty() { last.join("\n") } else { errors[errors.len().saturating_sub(4)..].join("\n") };
        return Err(format!("The engine stopped with an error (code {}).\n{detail}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bars_split_on_carriage_returns() {
        let mut buf = b"[I] loading\r\n  |==>   | 1/4 - 2.1it/s\x1b[K\r  |====> | 2/4 - 2.0it/s\x1b[K\rpartial".to_vec();
        let mut out = Vec::new();
        split_lines(&mut buf, &mut out);
        assert_eq!(out, vec!["[I] loading", "  |==>   | 1/4 - 2.1it/s", "  |====> | 2/4 - 2.0it/s"]);
        assert_eq!(buf, b"partial");
    }
}
