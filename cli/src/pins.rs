//! Pinned sessions: a list of ids in `<config>/sessile/pins.json`, kept apart
//! from the transcripts because Claude Code reads those and never this.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Kind, Result, SessileError};
use crate::paths::{exists_in, neither, validate_id};
use crate::session::Row;

#[derive(Serialize, Deserialize, Default)]
struct PinsFile {
    pinned: BTreeSet<String>,
}

pub fn pins_path(config: &Path) -> PathBuf {
    config.join("sessile").join("pins.json")
}

/// The pinned ids; none when the file does not exist yet.
pub fn load(config: &Path) -> Result<BTreeSet<String>> {
    let path = pins_path(config);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(SessileError::io("read", &path, &e)),
    };
    serde_json::from_str::<PinsFile>(&text)
        .map(|f| f.pinned)
        .map_err(|e| {
            SessileError::new(
                Kind::PinsUnreadable,
                format!("cannot parse {}: {e}", path.display()),
            )
            .with_hint(format!(
                "Fix {} or remove it to drop every pin",
                path.display()
            ))
            .with_context("path", serde_json::json!(path.display().to_string()))
        })
}

/// Written beside, under a name of this process, and renamed over, so a crash
/// never leaves half a file.
fn save(config: &Path, pinned: BTreeSet<String>) -> Result<()> {
    let path = pins_path(config);
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let io_err = |op: &str, p: &Path, e: io::Error| SessileError::io(op, p, &e);
    let text = serde_json::to_string_pretty(&PinsFile { pinned }).expect("a set of ids encodes");
    fs::write(&tmp, text + "\n").map_err(|e| io_err("write", &tmp, e))?;
    fs::rename(&tmp, &path).map_err(|e| io_err("rename", &path, e))
}

/// Reads the pins, lets `change` edit them and writes them back when it
/// changed anything, all under an exclusive lock on `pins.lock`: two commands
/// at once never lose each other's pin.
fn update(config: &Path, change: impl FnOnce(&mut BTreeSet<String>) -> bool) -> Result<bool> {
    let lock_path = pins_path(config).with_extension("lock");
    let io_err = |op: &str, p: &Path, e: io::Error| SessileError::io(op, p, &e);
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
    }
    let lock = fs::File::create(&lock_path).map_err(|e| io_err("create", &lock_path, e))?;
    lock.lock().map_err(|e| io_err("lock", &lock_path, e))?;
    let mut pinned = load(config)?;
    let changed = change(&mut pinned);
    if changed {
        save(config, pinned)?;
    }
    Ok(changed)
}

/// Pins a session that exists, live or archived. Whether it changed anything.
pub fn pin(config: &Path, id: &str, project: Option<&str>) -> Result<bool> {
    validate_id(id)?;
    if !exists_in(config, id, false, project) && !exists_in(config, id, true, project) {
        return Err(neither(id));
    }
    update(config, |pinned| pinned.insert(id.to_string()))
}

/// Unpins an id whether or not its session still exists.
pub fn unpin(config: &Path, id: &str) -> Result<bool> {
    validate_id(id)?;
    update(config, |pinned| pinned.remove(id))
}

pub fn mark<'a>(rows: impl IntoIterator<Item = &'a mut Row>, pinned: &BTreeSet<String>) {
    for row in rows {
        row.is_pinned = pinned.contains(&row.id);
    }
}

/// Pinned rows first; the sort is stable, so each group keeps its order.
pub fn pinned_first(rows: &mut [Row]) {
    rows.sort_by_key(|r| !r.is_pinned);
}
