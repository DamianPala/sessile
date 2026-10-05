//! Sessions queued to be deleted once nobody runs them, if they are still
//! empty then: the fresh session `/clear` leaves behind when the pane deletes
//! the session it was in. Kept in `<config>/sessile/pending-empty.json`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::json;

use crate::error::{Kind, Result, SessileError};
use crate::idfile::IdFile;
use crate::live::{guard, guard_others};
use crate::ops::{Ctx, Skipped, Target, delete_paths, remove_all};
use crate::paths::{Located, locate, validate_id};
use crate::pins;
use crate::scan::{Mapped, has_conversation, scan_transcript};

const PENDING: IdFile = IdFile {
    name: "pending-empty",
    key: "ids",
    unreadable: Kind::IoError,
};

/// Claude Code writes a session's last lines just after it lets go of it (a
/// few tens of milliseconds after its `sessions/<pid>.json` stops naming it);
/// a transcript written to more recently than this is left for the next sweep.
const QUIET: Duration = Duration::from_secs(2);

/// The queued ids; none when the file does not exist.
pub fn load(config: &Path) -> Result<BTreeSet<String>> {
    PENDING.load(config)
}

/// No counted prompt, no reply, no attachment: the rule of delete-empty.
fn is_empty(path: &Path) -> Result<bool> {
    let map = Mapped::open(path).map_err(|e| SessileError::io("read", path, &e))?;
    Ok(scan_transcript(map.bytes()).1.count == 0 && !has_conversation(map.bytes()))
}

/// Queues an empty session, the current one included, unless another Claude
/// Code runs it. The current session may have no transcript yet: Claude Code
/// writes the first lines of the one `/clear` opens a moment after the switch.
/// Whether it was not queued already.
pub fn enqueue(ctx: &Ctx, id: &str) -> Result<bool> {
    validate_id(id)?;
    let located = match locate(ctx.config, id, false, ctx.project) {
        Ok(located) => Some(located),
        Err(e) if e.kind == Kind::NotFound && ctx.current == Some(id) => None,
        Err(e) => return Err(e),
    };
    guard_others(ctx.config, id, ctx.current, "queue")?;
    let has_conversation = match &located {
        Some(located) => !is_empty(&located.transcript(id))?,
        None => false,
    };
    if has_conversation {
        return Err(SessileError::new(
            Kind::Conflict,
            format!("session {id} has a conversation, so it would never be deleted as empty"),
        )
        .with_hint(format!(
            "Delete it with `sessile delete {id}` once nobody runs it"
        ))
        .with_context("id", json!(id)));
    }
    PENDING.update(ctx.config, |ids| ids.insert(id.to_string()))
}

#[derive(Serialize, Debug, Default)]
pub struct Sweep {
    /// Deleted, or with --dry-run to be deleted.
    pub targets: Vec<Target>,
    /// Still queued: running, written to a moment ago, or not removable now.
    pub skipped: Vec<Skipped>,
    /// Taken off the queue and kept: gone already, given a conversation, pinned.
    pub dropped: Vec<Skipped>,
    pub changed: bool,
}

enum Verdict {
    Delete(Vec<PathBuf>),
    Skip(String),
    Drop(String),
}

fn age(path: &Path) -> Result<Duration> {
    let md = fs::metadata(path).map_err(|e| SessileError::io("stat", path, &e))?;
    let modified = md
        .modified()
        .map_err(|e| SessileError::io("stat", path, &e))?;
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO))
}

/// Running comes first: a session still in use may not have a transcript yet.
fn verdict(config: &Path, id: &str, current: Option<&str>, pinned: &BTreeSet<String>) -> Verdict {
    if let Err(e) = guard(config, id, current, "delete") {
        return Verdict::Skip(e.message);
    }
    let located = match locate(config, id, false, None) {
        Ok(located) => located,
        Err(e) if e.kind == Kind::NotFound => return Verdict::Drop("gone".into()),
        Err(e) => return Verdict::Skip(e.message),
    };
    match judge(config, &located, id, pinned) {
        Ok(verdict) => verdict,
        Err(e) => Verdict::Skip(e.message),
    }
}

fn judge(config: &Path, located: &Located, id: &str, pinned: &BTreeSet<String>) -> Result<Verdict> {
    let path = located.transcript(id);
    let age = age(&path)?;
    if age < QUIET {
        return Ok(Verdict::Skip(format!(
            "written to {} ms ago",
            age.as_millis()
        )));
    }
    if !is_empty(&path)? {
        return Ok(Verdict::Drop("has a conversation".into()));
    }
    if pinned.contains(id) {
        return Ok(Verdict::Drop("pinned".into()));
    }
    Ok(Verdict::Delete(delete_paths(config, located, id)?))
}

fn display(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

/// Deletes every queued session that nobody runs, that was not written to in
/// the last two seconds and that is still empty; drops from the queue the ones
/// that are gone or got a conversation. Runs under the queue's lock, so two
/// sweeps never remove the same session.
pub fn sweep(config: &Path, current: Option<&str>, dry_run: bool) -> Result<Sweep> {
    let pinned = pins::load(config)?;
    let mut report = Sweep::default();
    let mut judge_all = |ids: &BTreeSet<String>, remove: bool| -> Vec<String> {
        let mut done = Vec::new();
        for id in ids {
            match verdict(config, id, current, &pinned) {
                Verdict::Delete(paths) => {
                    if remove && let Err(e) = remove_all(id, &paths, &[]) {
                        report
                            .skipped
                            .push(skipped(id, format!("remove failed: {}", e.message)));
                        continue;
                    }
                    report.targets.push(Target {
                        id: id.clone(),
                        paths: display(&paths),
                    });
                    done.push(id.clone());
                }
                Verdict::Skip(reason) => report.skipped.push(skipped(id, reason)),
                Verdict::Drop(reason) => {
                    report.dropped.push(skipped(id, reason));
                    done.push(id.clone());
                }
            }
        }
        done
    };
    if dry_run {
        judge_all(&PENDING.load(config)?, false);
        return Ok(report);
    }
    // Nothing to do is the common case of the pane's timer: no lock, no file.
    if PENDING.load(config)?.is_empty() {
        return Ok(report);
    }
    let changed = PENDING.update(config, |ids| {
        let done = judge_all(ids, true);
        ids.retain(|id| !done.contains(id));
        !done.is_empty()
    })?;
    report.changed = changed;
    Ok(report)
}

fn skipped(id: &str, reason: String) -> Skipped {
    Skipped {
        id: id.to_string(),
        reason,
    }
}
