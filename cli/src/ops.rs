//! Commands that change sessions: rename, archive, restore, delete, delete-empty.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::error::{Kind, Result, SessileError};
use crate::export::cache_path;
use crate::interrupt::{self, Changing};
use crate::live::{guard, live_ids};
use crate::paths::{ARCHIVE_DIR, Located, exists_in, footprint, locate, neither, validate_id};
use crate::pins;
use crate::scan::{Mapped, find_titles, has_conversation};
use crate::session::{Row, list_project, session_id};

/// What every change command needs besides its own arguments.
pub struct Ctx<'a> {
    pub config: &'a Path,
    /// The session this process runs in; never changed.
    pub current: Option<&'a str>,
    /// Project slug that scopes the id lookup; `None` searches every project.
    pub project: Option<&'a str>,
}

/// `--dry-run` and `--yes` of the irreversible commands.
#[derive(Clone, Copy)]
pub struct Confirm {
    pub dry_run: bool,
    pub yes: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct Moved {
    pub from: String,
    pub to: String,
}

#[derive(Serialize)]
struct TitleEntry<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "customTitle")]
    custom_title: &'a str,
    #[serde(rename = "sessionId")]
    session_id: &'a str,
}

pub const MAX_TITLE_CHARS: usize = 200;

/// Appends the same `custom-title` entry `/rename` writes, unless the session
/// already carries that custom title. Whether it appended.
pub fn rename(ctx: &Ctx, id: &str, title: &str) -> Result<bool> {
    validate_id(id)?;
    let title = title.trim();
    if title.is_empty() {
        return Err(SessileError::usage("title must not be empty"));
    }
    // A title is one line in every list that shows it.
    if title.chars().any(char::is_control) {
        return Err(
            SessileError::usage("title must not hold line breaks or control characters")
                .with_hint("Pass the title as one line of plain text"),
        );
    }
    let length = title.chars().count();
    if length > MAX_TITLE_CHARS {
        return Err(SessileError::usage(format!(
            "title is {length} characters long, the limit is {MAX_TITLE_CHARS}"
        )));
    }
    let located = match locate(ctx.config, id, false, ctx.project) {
        Ok(located) => located,
        Err(_) if exists_in(ctx.config, id, true, ctx.project) => {
            return Err(SessileError::new(
                Kind::NotFound,
                format!("session {id} is archived, and an archived session is not renamed"),
            )
            .with_hint(format!("Restore it first: sessile restore {id}"))
            .with_context("id", json!(id)));
        }
        Err(e) if e.kind == Kind::NotFound => return Err(neither(id)),
        Err(e) => return Err(e),
    };
    guard(ctx.config, id, ctx.current, "rename")?;
    let path = located.transcript(id);
    let io_err = |op: &str, e: std::io::Error| SessileError::io(op, &path, &e);
    let map = Mapped::open(&path).map_err(|e| io_err("read", e))?;
    if find_titles(map.bytes()).custom.as_deref() == Some(title) {
        return Ok(false);
    }
    drop(map);
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|e| io_err("open", e))?;
    let len = file.metadata().map_err(|e| io_err("stat", e))?.len();
    let mut entry = String::new();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))
            .map_err(|e| io_err("seek", e))?;
        file.read_exact(&mut last).map_err(|e| io_err("read", e))?;
        if last[0] != b'\n' {
            entry.push('\n');
        }
    }
    let line = serde_json::to_string(&TitleEntry {
        kind: "custom-title",
        custom_title: title,
        session_id: id,
    })
    .expect("a title entry encodes");
    entry.push_str(&line);
    entry.push('\n');
    // A failed append may have written the entry, or a part of it.
    file.write_all(entry.as_bytes()).map_err(|e| {
        SessileError::new(
            Kind::OutcomeUnknown,
            format!("append {} failed: {e}", path.display()),
        )
        .with_hint(format!(
            "The title may or may not have changed: check with `sessile get {id}`"
        ))
        .with_context("id", json!(id))
        .with_context("path", json!(path.display().to_string()))
    })?;
    Ok(true)
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn rel<'a>(path: &'a Path, base: &Path) -> Result<&'a Path> {
    path.strip_prefix(base).map_err(|_| {
        SessileError::new(
            Kind::IoError,
            format!("{} is outside {}", path.display(), base.display()),
        )
    })
}

fn moved(pairs: &[&(PathBuf, PathBuf)]) -> Vec<Moved> {
    pairs
        .iter()
        .map(|(from, to)| Moved {
            from: from.display().to_string(),
            to: to.display().to_string(),
        })
        .collect()
}

fn move_one((from, to): &(PathBuf, PathBuf)) -> Result<()> {
    let result = match to.parent() {
        Some(parent) => fs::create_dir_all(parent).and_then(|()| fs::rename(from, to)),
        None => fs::rename(from, to),
    };
    result.map_err(|e| SessileError::io("move", from, &e))
}

/// Moves back what was moved; the moves it could not undo stay in place.
fn roll_back<'a>(done: &[&'a (PathBuf, PathBuf)]) -> Vec<&'a (PathBuf, PathBuf)> {
    done.iter()
        .rev()
        .filter(|(from, to)| fs::rename(to, from).is_err())
        .copied()
        .collect()
}

/// Renames every pair, or none: collisions refuse before the first move, and
/// a failed or interrupted move rolls the earlier ones back. Moves that cannot
/// be undone are named in the error's `context.completed`.
fn move_all(pairs: &[(PathBuf, PathBuf)], clash_hint: &str) -> Result<Vec<Moved>> {
    let clashes: Vec<String> = pairs
        .iter()
        .filter(|(_, to)| exists(to))
        .map(|(_, to)| to.display().to_string())
        .collect();
    if !clashes.is_empty() {
        return Err(
            SessileError::new(Kind::Conflict, "destination already exists")
                .with_hint(clash_hint)
                .with_context("paths", json!(clashes)),
        );
    }
    let _changing = Changing::begin();
    let mut done: Vec<&(PathBuf, PathBuf)> = Vec::new();
    for pair in pairs {
        if let Err(e) = interrupt::check().and_then(|()| move_one(pair)) {
            let stuck = roll_back(&done);
            if stuck.is_empty() {
                return Err(e);
            }
            let message = format!(
                "{}; {} earlier moves could not be undone",
                e.message,
                stuck.len()
            );
            return Err(
                SessileError { message, ..e }.with_context("completed", json!(moved(&stuck)))
            );
        }
        done.push(pair);
    }
    Ok(moved(&done))
}

/// Both places hold files of one session when Claude Code wrote to a session
/// after it was archived: the two copies are two parts of one conversation.
const SPLIT_HINT: &str = "Both projects and the archive hold files of this session, as happens \
    when Claude Code writes to a session after it was archived: compare the listed paths with \
    their counterparts, move one copy away, then repeat the command";

/// Moves the footprint under `session-archive/<slug>/`, keeping relative
/// paths. An already archived session moves nothing.
pub fn archive(ctx: &Ctx, id: &str) -> Result<Vec<Moved>> {
    let config = ctx.config;
    let located = match locate(config, id, false, ctx.project) {
        Ok(located) => located,
        Err(_) if exists_in(config, id, true, ctx.project) => return Ok(Vec::new()),
        Err(e) if e.kind == Kind::NotFound => return Err(neither(id)),
        Err(e) => return Err(e),
    };
    guard(config, id, ctx.current, "archive")?;
    let dest_root = config.join(ARCHIVE_DIR).join(&located.slug);
    let pairs = footprint(config, &located.slug, id)?
        .into_iter()
        .map(|from| {
            let to = dest_root.join(rel(&from, config)?);
            Ok((from, to))
        })
        .collect::<Result<Vec<_>>>()?;
    move_all(&pairs, SPLIT_HINT)
}

/// Moves an archived footprint back; refuses if any target path exists. A
/// session already out of the archive moves nothing.
pub fn restore(ctx: &Ctx, id: &str) -> Result<Vec<Moved>> {
    let config = ctx.config;
    let located = match locate(config, id, true, ctx.project) {
        Ok(located) => located,
        Err(_) if exists_in(config, id, false, ctx.project) => return Ok(Vec::new()),
        Err(e) if e.kind == Kind::NotFound => return Err(neither(id)),
        Err(e) => return Err(e),
    };
    guard(config, id, ctx.current, "restore")?;
    let pairs = footprint(&located.root, &located.slug, id)?
        .into_iter()
        .map(|from| {
            let to = config.join(rel(&from, &located.root)?);
            Ok((from, to))
        })
        .collect::<Result<Vec<_>>>()?;
    let moved = move_all(&pairs, SPLIT_HINT)?;
    let sources: Vec<PathBuf> = pairs.into_iter().map(|(from, _)| from).collect();
    prune_empty_dirs(&sources, &located.root);
    Ok(moved)
}

/// Removes directories that moving or deleting `sources` emptied, up to and
/// including the archive project dir `root`. `remove_dir` refuses non-empty dirs.
fn prune_empty_dirs(sources: &[PathBuf], root: &Path) {
    let stop = root.parent();
    for source in sources {
        let mut dir = source.parent();
        while let Some(d) = dir {
            if Some(d) == stop || fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

/// One session a delete removes, or would remove, and its paths.
#[derive(Serialize, Debug, Clone)]
pub struct Target {
    pub id: String,
    pub paths: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct Deletion {
    pub targets: Vec<Target>,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_confirmation: Option<bool>,
}

fn remove_path(path: &Path) -> Result<()> {
    let md = fs::symlink_metadata(path).map_err(|e| SessileError::io("stat", path, &e))?;
    let result = if md.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|e| SessileError::io("remove", path, &e))
}

fn display_paths(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

/// The footprint plus a full export sessile kept for this session; the
/// transcript stays last, so a delete that stops half way can be repeated.
fn delete_paths(config: &Path, located: &Located, id: &str) -> Result<Vec<PathBuf>> {
    let mut paths = footprint(&located.root, &located.slug, id)?;
    let cache = cache_path(config, id);
    if exists(&cache) {
        paths.insert(paths.len().saturating_sub(1), cache);
    }
    Ok(paths)
}

/// Removes `paths` in order. A failure or Ctrl-C stops it; the error carries
/// what was removed before, on top of `done`, in `context.completed`.
fn remove_all(id: &str, paths: &[PathBuf], done: &[Target]) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        if let Err(e) = interrupt::check().and_then(|()| remove_path(path)) {
            let mut completed = done.to_vec();
            if i > 0 {
                completed.push(Target {
                    id: id.to_string(),
                    paths: display_paths(&paths[..i]),
                });
            }
            return Err(if completed.is_empty() {
                e
            } else {
                e.with_context("completed", json!(completed))
            });
        }
    }
    Ok(())
}

fn confirmation_required(message: String) -> SessileError {
    SessileError::new(Kind::ConfirmationRequired, message)
        .with_hint("Repeat with --yes; --dry-run lists what would be removed")
}

/// Removes exactly the paths `--dry-run` lists, and the session's pin.
pub fn delete(ctx: &Ctx, id: &str, archived: bool, confirm: Confirm) -> Result<Deletion> {
    let located = locate(ctx.config, id, archived, ctx.project)?;
    guard(ctx.config, id, ctx.current, "delete")?;
    // An unreadable pins file refuses here, before anything is removed.
    pins::load(ctx.config)?;
    let paths = delete_paths(ctx.config, &located, id)?;
    let target = Target {
        id: id.to_string(),
        paths: display_paths(&paths),
    };
    if confirm.dry_run {
        return Ok(Deletion {
            targets: vec![target],
            changed: false,
            requires_confirmation: Some(true),
        });
    }
    if !confirm.yes {
        return Err(confirmation_required(format!(
            "deleting session {id} cannot be undone and needs confirmation"
        ))
        .with_context("id", json!(id)));
    }
    let _changing = Changing::begin();
    remove_all(id, &paths, &[])?;
    if located.archived {
        prune_empty_dirs(&paths, &located.root);
    }
    pins::unpin(ctx.config, id)?;
    Ok(Deletion {
        targets: vec![target],
        changed: true,
        requires_confirmation: None,
    })
}

#[derive(Serialize, Debug)]
pub struct EmptyTarget {
    #[serde(flatten)]
    pub row: Row,
    pub paths: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct Skipped {
    pub id: String,
    pub reason: String,
}

#[derive(Serialize, Debug)]
pub struct EmptyReport {
    pub targets: Vec<EmptyTarget>,
    pub skipped: Vec<Skipped>,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_confirmation: Option<bool>,
}

/// The empty sessions of a project (no prompt, no reply, no attachment) that
/// may go, and the ones that stay: pinned, current, live, or unreadable
/// (their content is unknown).
fn empty_targets(
    config: &Path,
    slug: &str,
    current: Option<&str>,
) -> Result<(Vec<EmptyTarget>, Vec<Skipped>)> {
    let pinned = pins::load(config)?;
    let (rows, unreadable) = list_project(config, slug, false, &live_ids(config));
    let located = Located {
        root: config.to_path_buf(),
        slug: slug.to_string(),
        archived: false,
    };
    let mut skipped: Vec<Skipped> = unreadable
        .into_iter()
        .filter_map(|u| {
            let id = session_id(Path::new(&u.path))?;
            Some(Skipped {
                id,
                reason: format!("unreadable: {}", u.message),
            })
        })
        .collect();
    let mut targets = Vec::new();
    for row in rows.into_iter().filter(|r| r.prompts == 0) {
        // No counted prompt is not yet empty: a slash command or a screenshot
        // starts a conversation without one.
        match Mapped::open(&located.transcript(&row.id)) {
            Ok(map) if has_conversation(map.bytes()) => continue,
            Ok(_) => {}
            Err(e) => {
                skipped.push(Skipped {
                    id: row.id,
                    reason: format!("unreadable: {e}"),
                });
                continue;
            }
        }
        if pinned.contains(&row.id) {
            skipped.push(Skipped {
                id: row.id,
                reason: "pinned".into(),
            });
            continue;
        }
        if let Err(e) = guard(config, &row.id, current, "delete") {
            skipped.push(Skipped {
                id: row.id,
                reason: e.message,
            });
            continue;
        }
        let paths = display_paths(&delete_paths(config, &located, &row.id)?);
        targets.push(EmptyTarget { row, paths });
    }
    Ok((targets, skipped))
}

/// Deletes every empty session of a project. Current, live, pinned and
/// unreadable sessions are skipped (and reported) instead of failing the run.
pub fn delete_empty(
    config: &Path,
    slug: &str,
    current: Option<&str>,
    confirm: Confirm,
) -> Result<EmptyReport> {
    let (targets, mut skipped) = empty_targets(config, slug, current)?;
    let report = |targets, skipped, changed, requires_confirmation| EmptyReport {
        targets,
        skipped,
        changed,
        requires_confirmation,
    };
    if confirm.dry_run {
        let gated = !targets.is_empty();
        return Ok(report(targets, skipped, false, Some(gated)));
    }
    if targets.is_empty() {
        return Ok(report(targets, skipped, false, None));
    }
    if !confirm.yes {
        return Err(confirmation_required(format!(
            "deleting {} empty sessions cannot be undone and needs confirmation",
            targets.len()
        ))
        .with_context("count", json!(targets.len())));
    }
    let _changing = Changing::begin();
    let mut done: Vec<Target> = Vec::new();
    let mut removed = Vec::new();
    for target in targets {
        // Asked again right before it goes: a session chosen a moment ago may
        // have been resumed since.
        let id = &target.row.id;
        if let Err(e) = guard(config, id, current, "delete") {
            skipped.push(Skipped {
                id: target.row.id,
                reason: e.message,
            });
            continue;
        }
        let paths: Vec<PathBuf> = target.paths.iter().map(PathBuf::from).collect();
        remove_all(id, &paths, &done)?;
        done.push(Target {
            id: id.clone(),
            paths: target.paths.clone(),
        });
        removed.push(target);
    }
    let changed = !removed.is_empty();
    Ok(report(removed, skipped, changed, None))
}
