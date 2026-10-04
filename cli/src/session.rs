//! Session rows: what `list`, `show` and `search` report about one transcript.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use serde::Serialize;

use crate::error::{Kind, Result, SessileError};
use crate::output::rfc3339_ms;
use crate::paths::{Located, project_dir, resume_command, validate_id};
use crate::scan::{
    Mapped, PromptStats, TITLE_KEEP, Titles, find_head, find_meta, scan_transcript, truncate_chars,
};

#[derive(Serialize, Debug, Clone)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub title_source: &'static str,
    /// Transcript mtime; sorting uses it, the output carries `updated_at`.
    #[serde(skip)]
    pub updated_ms: u64,
    pub updated_at: String,
    /// When the session started, from its first entry with a `cwd`.
    pub created_at: Option<String>,
    pub size_bytes: u64,
    pub prompts: usize,
    pub is_live: bool,
    /// The session this process runs in (`--current`).
    pub is_current: bool,
    pub is_pinned: bool,
    pub first_prompt: Option<String>,
    /// Directory the session ran in, from the transcript.
    pub cwd: Option<String>,
    pub resume_command: String,
    /// False for sessions started by `claude -p` or the Agent SDK, None when
    /// the transcript does not say.
    pub interactive: Option<bool>,
}

/// A transcript that exists but could not be read; reported, never guessed at.
#[derive(Serialize, Debug, Clone)]
pub struct Unreadable {
    pub path: String,
    pub message: String,
}

#[derive(Serialize, Debug)]
pub struct Detail {
    #[serde(flatten)]
    pub row: Row,
    pub first_prompts: Vec<String>,
    pub last_prompts: Vec<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub version: Option<String>,
    pub path: String,
}

/// custom title, else AI title, else the first prompt, else nothing.
pub fn resolve_title(titles: &Titles, stats: &PromptStats) -> (String, &'static str) {
    if let Some(t) = &titles.custom {
        (t.clone(), "custom")
    } else if let Some(t) = &titles.ai {
        (t.clone(), "ai")
    } else if let Some(p) = stats.first.first() {
        (truncate_chars(p, TITLE_KEEP), "prompt")
    } else {
        (String::new(), "none")
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn mtime_ms(md: &Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A stray `*.jsonl` with a non-UUID name is not a session: its stem would
/// widen the footprint globs (`security/*<id>*`) and land unquoted in the
/// resume command.
pub fn session_id(path: &Path) -> Option<String> {
    let id = path.file_stem()?.to_str()?;
    validate_id(id).ok()?;
    Some(id.to_string())
}

/// What a row is built from besides the file itself.
pub struct Scanned<'a> {
    pub titles: &'a Titles,
    pub stats: &'a PromptStats,
    pub cwd: Option<String>,
    pub interactive: Option<bool>,
    pub created_at: Option<String>,
}

pub fn build_row(
    path: &Path,
    md: &Metadata,
    scanned: Scanned,
    live: &HashSet<String>,
) -> Option<Row> {
    let id = session_id(path)?;
    let (title, title_source) = resolve_title(scanned.titles, scanned.stats);
    let is_live = live.contains(&id);
    let updated_ms = mtime_ms(md);
    Some(Row {
        title,
        title_source,
        updated_ms,
        updated_at: rfc3339_ms(updated_ms),
        created_at: scanned.created_at,
        size_bytes: md.len() + dir_size(&path.with_extension("")),
        prompts: scanned.stats.count,
        is_live,
        is_current: false,
        is_pinned: false,
        first_prompt: scanned.stats.first.first().cloned(),
        resume_command: resume_command(&id, scanned.cwd.as_deref()),
        cwd: scanned.cwd,
        interactive: scanned.interactive,
        id,
    })
}

fn summarize(path: &Path, live: &HashSet<String>) -> io::Result<Option<Row>> {
    let md = fs::metadata(path)?;
    let map = Mapped::open(path)?;
    let (titles, stats) = scan_transcript(map.bytes());
    let head = find_head(map.bytes());
    let scanned = Scanned {
        titles: &titles,
        stats: &stats,
        cwd: head.cwd,
        interactive: head.interactive,
        created_at: head.created_at,
    };
    Ok(build_row(path, &md, scanned, live))
}

/// All `*.jsonl` transcripts of a project directory.
pub fn transcripts(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect()
}

pub fn sort_newest_first(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        b.updated_ms
            .cmp(&a.updated_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Transcripts of the given projects, live or archived.
pub fn transcripts_of(config: &Path, slugs: &[String], archived: bool) -> Vec<PathBuf> {
    slugs
        .iter()
        .flat_map(|slug| transcripts(&project_dir(config, slug, archived)))
        .collect()
}

/// What one transcript yielded: a row, nothing (gone or not a session), or a read error.
pub enum Scan<T> {
    Found(T),
    Skipped,
    Unreadable(Unreadable),
}

impl<T> Scan<T> {
    /// A file that vanished between listing and reading is not a session any
    /// more; any other failure is reported.
    pub fn from_io(path: &Path, result: io::Result<Option<T>>) -> Self {
        match result {
            Ok(Some(found)) => Scan::Found(found),
            Ok(None) => Scan::Skipped,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Scan::Skipped,
            Err(e) => Scan::Unreadable(Unreadable {
                path: path.display().to_string(),
                message: e.to_string(),
            }),
        }
    }
}

/// Splits scan results into what was found and what could not be read.
pub fn split<T>(scans: Vec<Scan<T>>) -> (Vec<T>, Vec<Unreadable>) {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for scan in scans {
        match scan {
            Scan::Found(f) => found.push(f),
            Scan::Skipped => {}
            Scan::Unreadable(u) => unreadable.push(u),
        }
    }
    unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    (found, unreadable)
}

/// One row per session of the given projects, newest first, and the
/// transcripts that could not be read.
pub fn list_projects(
    config: &Path,
    slugs: &[String],
    archived: bool,
    live: &HashSet<String>,
) -> (Vec<Row>, Vec<Unreadable>) {
    let scans: Vec<Scan<Row>> = transcripts_of(config, slugs, archived)
        .par_iter()
        .map(|p| Scan::from_io(p, summarize(p, live)))
        .collect();
    let (mut rows, unreadable) = split(scans);
    sort_newest_first(&mut rows);
    (rows, unreadable)
}

pub fn list_project(
    config: &Path,
    slug: &str,
    archived: bool,
    live: &HashSet<String>,
) -> (Vec<Row>, Vec<Unreadable>) {
    list_projects(config, &[slug.to_string()], archived, live)
}

/// Preview of one session: its row plus first/last prompts and metadata.
pub fn show(located: &Located, id: &str, live: &HashSet<String>) -> Result<Detail> {
    let path = located.transcript(id);
    let io_err = |op: &str, e: io::Error| SessileError::io(op, &path, &e);
    let md = fs::metadata(&path).map_err(|e| io_err("stat", e))?;
    let map = Mapped::open(&path).map_err(|e| io_err("read", e))?;
    let (titles, stats) = scan_transcript(map.bytes());
    let meta = find_meta(map.bytes());
    let scanned = Scanned {
        titles: &titles,
        stats: &stats,
        cwd: meta.cwd,
        interactive: meta.interactive,
        created_at: meta.created_at,
    };
    let row = build_row(&path, &md, scanned, live)
        .ok_or_else(|| SessileError::new(Kind::NotFound, format!("session {id} not found")))?;
    Ok(Detail {
        row,
        first_prompts: stats.first,
        last_prompts: stats.last.into_iter().collect(),
        git_branch: meta.git_branch,
        model: meta.model,
        version: meta.version,
        path: path.display().to_string(),
    })
}
