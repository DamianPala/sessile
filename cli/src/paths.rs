//! Locations under the Claude config dir: project slugs, session lookup, footprint.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Kind, Result, SessileError};

pub const ARCHIVE_DIR: &str = "session-archive";

/// `CLAUDE_CONFIG_DIR`, else `.claude` in the home dir (`USERPROFILE` on Windows,
/// where Claude Code looks too).
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    match std::env::home_dir() {
        Some(home) if !home.as_os_str().is_empty() => Ok(home.join(".claude")),
        _ => Err(
            SessileError::usage("no home directory and no CLAUDE_CONFIG_DIR")
                .with_hint("Set CLAUDE_CONFIG_DIR to the Claude Code config directory"),
        ),
    }
}

/// A leading `~` is the home dir: the value comes from a settings field, where
/// no shell expands it.
pub fn expand_home(dir: &str) -> PathBuf {
    let rest = dir
        .strip_prefix("~/")
        .or_else(|| dir.strip_prefix(r"~\"))
        .or((dir == "~").then_some(""));
    match (rest, std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(dir),
    }
}

/// Every project slug that has a directory, live or in the archive, sorted.
pub fn project_slugs(config: &Path, archived: bool) -> Vec<String> {
    let base = config.join(if archived { ARCHIVE_DIR } else { "projects" });
    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };
    let mut slugs: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    slugs.sort();
    slugs
}

/// What a person pastes into their shell to resume a session where it ran:
/// POSIX quoting, or PowerShell's on Windows.
pub fn resume_command(id: &str, cwd: Option<&str>) -> String {
    let Some(dir) = cwd else {
        return format!("claude --resume {id}");
    };
    if cfg!(windows) {
        format!("cd '{}'; claude --resume {id}", powershell_quoted(dir))
    } else {
        format!(
            "cd '{}' && claude --resume {id}",
            dir.replace('\'', r"'\''")
        )
    }
}

/// PowerShell ends a single-quoted string at the typographic single quotes
/// too, so each of them is doubled like `'`.
fn powershell_quoted(dir: &str) -> String {
    let mut out = String::with_capacity(dir.len());
    for c in dir.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out
}

/// The path as Claude Code saw it: `canonicalize` on Windows adds a `\\?\`
/// prefix that the process cwd never has.
fn plain(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// Project dir name Claude Code derives from a cwd: every non-alphanumeric
/// UTF-16 unit becomes `-` (so `/`, `.`, `_` and spaces alike).
pub fn slug_for(dir: &str) -> String {
    let mut slug = String::with_capacity(dir.len());
    for c in dir.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else {
            slug.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    slug
}

/// Slug of a `--project` argument. Relative paths are made absolute; when the
/// literal path has no project dir but its canonical form has, the canonical
/// form wins (symlinked cwd).
pub fn project_slug(config: &Path, dir: &str) -> Result<String> {
    let mut path = PathBuf::from(dir);
    if path.is_relative() {
        let cwd =
            std::env::current_dir().map_err(|e| SessileError::io("current_dir", &path, &e))?;
        path = cwd.join(path);
    }
    let literal = plain(&path);
    let slug = slug_for(literal.trim_end_matches(['/', '\\']));
    if config.join("projects").join(&slug).is_dir() {
        return Ok(slug);
    }
    if let Ok(canon) = fs::canonicalize(&path) {
        let canon_slug = slug_for(&plain(&canon));
        if config.join("projects").join(&canon_slug).is_dir() {
            return Ok(canon_slug);
        }
    }
    Ok(slug)
}

/// Session ids are UUIDs. Footprint globs (`security/*<id>*`) are only safe
/// with a full id, so anything else is a usage error.
pub fn validate_id(id: &str) -> Result<()> {
    let groups: Vec<&str> = id.split('-').collect();
    let shape_ok = groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(n, g)| g.len() == *n && g.bytes().all(|b| b.is_ascii_hexdigit()));
    if shape_ok {
        Ok(())
    } else {
        Err(
            SessileError::usage(format!("'{id}' is not a session id (expected a UUID)"))
                .with_hint("Pass the full id from `sessile list`"),
        )
    }
}

/// Root that holds a session's files: the config dir, or one archive project.
#[derive(Debug, Clone)]
pub struct Located {
    pub root: PathBuf,
    pub slug: String,
    pub archived: bool,
}

impl Located {
    pub fn transcript(&self, id: &str) -> PathBuf {
        self.root
            .join("projects")
            .join(&self.slug)
            .join(format!("{id}.jsonl"))
    }
}

/// Directory that holds `<id>.jsonl` files for a project.
pub fn project_dir(config: &Path, slug: &str, archived: bool) -> PathBuf {
    if archived {
        config
            .join(ARCHIVE_DIR)
            .join(slug)
            .join("projects")
            .join(slug)
    } else {
        config.join("projects").join(slug)
    }
}

/// Finds the project that owns `<id>.jsonl`, in live projects or the archive.
/// With `only` (a project slug) just that project is looked at.
pub fn locate(config: &Path, id: &str, archived: bool, only: Option<&str>) -> Result<Located> {
    validate_id(id)?;
    let (base, nested) = if archived {
        (config.join(ARCHIVE_DIR), true)
    } else {
        (config.join("projects"), false)
    };
    let slugs: Vec<String> = match only {
        Some(slug) => vec![slug.to_string()],
        None => project_slugs(config, archived),
    };
    for slug in slugs {
        let root = if nested {
            base.join(&slug)
        } else {
            config.to_path_buf()
        };
        let located = Located {
            root,
            slug,
            archived,
        };
        if located.transcript(id).is_file() {
            return Ok(located);
        }
    }
    Err(not_found(id, archived))
}

/// The error of a command that looked in both places.
pub fn neither(id: &str) -> SessileError {
    SessileError::new(
        Kind::NotFound,
        format!("session {id} is neither in projects nor in the archive"),
    )
    .with_hint("Find the id with `sessile list --all` or `sessile list --all --archived`")
    .with_context("id", serde_json::json!(id))
}

fn not_found(id: &str, archived: bool) -> SessileError {
    let (place, hint) = if archived {
        ("the archive", "Drop --archived to look among live sessions")
    } else {
        ("projects", "Add --archived to look in the archive")
    };
    SessileError::new(Kind::NotFound, format!("session {id} not found in {place}"))
        .with_hint(hint)
        .with_context("id", serde_json::json!(id))
}

/// Whether `<id>.jsonl` exists in the live projects or in the archive.
pub fn exists_in(config: &Path, id: &str, archived: bool, only: Option<&str>) -> bool {
    locate(config, id, archived, only).is_ok()
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The entries of `dir` whose name `keep` takes; none when `dir` is absent.
/// A directory that cannot be listed is an error: its files would be missed.
fn names_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(SessileError::io("list", dir, &e)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| SessileError::io("list", dir, &e))?;
        if entry.file_name().to_str().is_some_and(&keep) {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

/// Every existing path keyed by the session id under `root`: the side files
/// sorted, the transcript last, so a change that stops half way leaves a
/// session that is still found. Never descends into `projects/<slug>/memory/`
/// or anything not named after the id.
pub fn footprint(root: &Path, slug: &str, id: &str) -> Result<Vec<PathBuf>> {
    let project = root.join("projects").join(slug);
    let transcript = project.join(format!("{id}.jsonl"));
    let fixed = [
        project.join(id),
        root.join("session-env").join(id),
        root.join("file-history").join(id),
        root.join("tasks").join(id),
        root.join("debug").join(format!("{id}.txt")),
    ];
    let mut paths: Vec<PathBuf> = fixed.into_iter().filter(|p| exists(p)).collect();
    let todo_prefix = format!("{id}-");
    paths.extend(names_in(&root.join("todos"), |n| {
        n.starts_with(&todo_prefix) && n.ends_with(".json")
    })?);
    paths.extend(names_in(&root.join("security"), |n| n.contains(id))?);
    paths.extend(names_in(&root.join("telemetry"), |n| n.contains(id))?);
    paths.sort();
    paths.dedup();
    if exists(&transcript) {
        paths.push(transcript);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_every_non_alphanumeric() {
        assert_eq!(slug_for("/home/me/ai"), "-home-me-ai");
        assert_eq!(slug_for("/home/me/.agents/x_y z"), "-home-me--agents-x-y-z");
        assert_eq!(slug_for("/tmp/a\u{105}b"), "-tmp-a-b");
    }

    #[test]
    fn windows_paths_slug_like_claude_code() {
        assert_eq!(slug_for(r"C:\Users\Ann\my.proj"), "C--Users-Ann-my-proj");
        assert_eq!(plain(Path::new(r"\\?\C:\Users\Ann")), r"C:\Users\Ann");
    }

    #[test]
    fn tilde_is_the_home_dir() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_home("~/x"), home.join("x"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/a/~/b"), PathBuf::from("/a/~/b"));
    }

    #[test]
    fn resume_command_quotes_the_dir() {
        assert_eq!(resume_command("x", None), "claude --resume x");
        let cmd = resume_command("x", Some("/a/it's"));
        if cfg!(windows) {
            assert_eq!(cmd, "cd '/a/it''s'; claude --resume x");
        } else {
            assert_eq!(cmd, r"cd '/a/it'\''s' && claude --resume x");
        }
    }

    #[test]
    fn powershell_quoting_doubles_typographic_quotes() {
        assert_eq!(powershell_quoted("/a/it's"), "/a/it''s");
        assert_eq!(
            powershell_quoted("/a/it\u{2019}s"),
            "/a/it\u{2019}\u{2019}s"
        );
    }

    #[test]
    fn id_must_be_a_uuid() {
        assert!(validate_id("0a1b2c3d-1111-4222-8333-444455556666").is_ok());
        assert!(validate_id("abc").is_err());
        assert!(validate_id("../../etc/passwdxxxxxxxxxxxxxxxxxxxxxxxxxx").is_err());
    }
}
