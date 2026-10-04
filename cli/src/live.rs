//! Live-session detection and the guard shared by every destructive command.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Kind, Result, SessileError};

#[derive(Deserialize)]
struct SessionFile {
    pid: Option<u32>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "procStart")]
    proc_start: Option<serde_json::Value>,
}

/// What `sessions/` tells about running Claude Code processes.
#[derive(Default)]
pub struct Live {
    /// Ids, in lower case, of the sessions a running process holds.
    pub ids: HashSet<String>,
    /// Running processes whose `sessions/<pid>.json` does not name a session:
    /// each holds a session nobody can tell.
    pub unknown: Vec<u32>,
}

/// A file caught while Claude Code rewrites it reads as garbage for a moment.
const READ_ATTEMPTS: u32 = 3;
const READ_PAUSE: Duration = Duration::from_millis(15);

fn named_session(path: &Path) -> Option<(u32, String, Option<String>)> {
    let file = serde_json::from_slice::<SessionFile>(&fs::read(path).ok()?).ok()?;
    let start = file.proc_start.as_ref().map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    Some((file.pid?, file.session_id?, start))
}

/// Reads `sessions/<pid>.json` of every process: the file names the session,
/// and the pid must be alive (and, when recorded, started at the recorded
/// time). A file that names no session counts as an unknown live session when
/// the pid in its name is alive.
pub fn live_sessions(config: &Path) -> Live {
    let mut live = Live::default();
    let Ok(entries) = fs::read_dir(config.join("sessions")) else {
        return live;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let owner = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|pid| pid_alive(*pid, None));
        let mut named = named_session(&path);
        for _ in 1..READ_ATTEMPTS {
            if named.is_some() || owner.is_none() {
                break;
            }
            thread::sleep(READ_PAUSE);
            named = named_session(&path);
        }
        match (named, owner) {
            (Some((pid, id, start)), _) => {
                if pid_alive(pid, start.as_deref()) {
                    live.ids.insert(id.to_ascii_lowercase());
                }
            }
            (None, Some(pid)) => live.unknown.push(pid),
            (None, None) => {}
        }
    }
    live
}

/// Session ids that have a running process.
pub fn live_ids(config: &Path) -> HashSet<String> {
    live_sessions(config).ids
}

#[cfg(target_os = "linux")]
fn pid_alive(pid: u32, proc_start: Option<&str>) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Fields after the parenthesised command name; state is the first, starttime the 20th.
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return false;
    }
    match proc_start {
        None => true,
        Some(want) => fields.get(19) == Some(&want),
    }
}

/// Without /proc the pid check is `kill -0`; the recorded start time is not verified.
#[cfg(all(unix, not(target_os = "linux")))]
fn pid_alive(pid: u32, _proc_start: Option<&str>) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A process handle that still reports `STILL_ACTIVE`; the recorded start time
/// is not verified.
#[cfg(windows)]
fn pid_alive(pid: u32, _proc_start: Option<&str>) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain Win32 calls on a handle owned and closed here.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
}

/// Refuses a change to the current or a live session. Called right before the
/// change, with a fresh read of the live sessions.
pub fn guard(config: &Path, id: &str, current: Option<&str>, action: &str) -> Result<()> {
    let refused = |message: String, hint: &str| {
        Err(SessileError::new(Kind::SessionLive, message)
            .with_hint(hint)
            .with_context("id", serde_json::json!(id)))
    };
    if current == Some(id) {
        return refused(
            format!("cannot {action} the current session {id}"),
            "Run the command from another session",
        );
    }
    let live = live_sessions(config);
    if live.ids.contains(id) {
        return refused(
            format!("cannot {action} live session {id}: a Claude Code process is running it"),
            "Exit that Claude Code session first",
        );
    }
    if let Some(pid) = live.unknown.first() {
        return refused(
            format!(
                "cannot {action} session {id}: process {pid} is running a session that \
                 sessions/{pid}.json does not name, and it may be this one"
            ),
            "Repeat the command; if it stays, exit that process or remove its stale file",
        );
    }
    Ok(())
}
