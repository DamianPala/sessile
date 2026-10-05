//! Pinned sessions: a list of ids in `<config>/sessile/pins.json`, kept apart
//! from the transcripts because Claude Code reads those and never this.

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::{Kind, Result};
use crate::idfile::IdFile;
use crate::paths::{exists_in, neither, validate_id};
use crate::session::Row;

const PINS: IdFile = IdFile {
    name: "pins",
    key: "pinned",
    unreadable: Kind::PinsUnreadable,
};

/// The pinned ids; none when the file does not exist yet.
pub fn load(config: &Path) -> Result<BTreeSet<String>> {
    PINS.load(config)
}

/// Pins a session that exists, live or archived. Whether it changed anything.
pub fn pin(config: &Path, id: &str, project: Option<&str>) -> Result<bool> {
    validate_id(id)?;
    if !exists_in(config, id, false, project) && !exists_in(config, id, true, project) {
        return Err(neither(id));
    }
    PINS.update(config, |pinned| pinned.insert(id.to_string()))
}

/// Unpins an id whether or not its session still exists.
pub fn unpin(config: &Path, id: &str) -> Result<bool> {
    validate_id(id)?;
    PINS.update(config, |pinned| pinned.remove(id))
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
