//! Markdown rendering of one session: the prompts and replies, tool calls
//! counted, tool output left out.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Result, SessileError};
use crate::paths::Located;
use crate::scan::{Mapped, turns};
use crate::session::{Detail, show};
use crate::turns::{Exchange, Selection, exchanges, select};

/// Largest `markdown` value inline in JSON; a longer one is cut there and
/// kept whole in `cache_path`.
pub const INLINE_MAX_BYTES: usize = 256 * 1024;

/// The JSON form of an export.
#[derive(Serialize, Debug)]
pub struct Document {
    pub id: String,
    pub markdown: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
}

/// Where the whole Markdown of a cut JSON export is kept: one file per
/// session, replaced by the next cut export and removed with the session.
pub fn cache_path(config: &Path, id: &str) -> PathBuf {
    config
        .join("sessile")
        .join("exports")
        .join(format!("{id}.md"))
}

/// The Markdown inline when it fits, else its first `INLINE_MAX_BYTES` and
/// the whole text in `cache_path`.
pub fn document(config: &Path, id: &str, markdown: String) -> Result<Document> {
    if markdown.len() <= INLINE_MAX_BYTES {
        return Ok(Document {
            id: id.to_string(),
            markdown,
            truncated: false,
            output_file: None,
        });
    }
    let path = cache_path(config, id);
    write_private(&path, &markdown)?;
    let mut end = INLINE_MAX_BYTES;
    while !markdown.is_char_boundary(end) {
        end -= 1;
    }
    Ok(Document {
        id: id.to_string(),
        markdown: markdown[..end].to_string(),
        truncated: true,
        output_file: Some(path.display().to_string()),
    })
}

/// Written beside and renamed over, readable by its owner only on Unix.
fn write_private(path: &Path, text: &str) -> Result<()> {
    let io_err = |op: &str, p: &Path, e: std::io::Error| SessileError::io(op, p, &e);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
    }
    let tmp = path.with_extension("md.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp).map_err(|e| io_err("create", &tmp, e))?;
    file.write_all(text.as_bytes())
        .map_err(|e| io_err("write", &tmp, e))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| io_err("rename", path, e))
}

fn header(d: &Detail, turns: Option<&str>) -> String {
    let title = if d.row.title.is_empty() {
        "(untitled)"
    } else {
        &d.row.title
    };
    let mut out = format!("# {title}\n\n- Session: `{}`\n", d.row.id);
    let fields = [
        ("Directory", d.row.cwd.as_deref()),
        ("Branch", d.git_branch.as_deref()),
        ("Model", d.model.as_deref()),
    ];
    for (label, value) in fields {
        if let Some(v) = value {
            out.push_str(&format!("- {label}: `{v}`\n"));
        }
    }
    out.push_str(&format!(
        "- Prompts: {}\n- Resume: `{}`\n",
        d.row.prompts, d.row.resume_command
    ));
    if let Some(turns) = turns {
        out.push_str(&format!("- Turns: {turns}\n"));
    }
    out.push_str("\n---\n");
    out
}

/// `Bash ×3, Read`, in order of first use.
fn tool_summary(tools: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        match counts.iter_mut().find(|(name, _)| name == tool) {
            Some((_, n)) => *n += 1,
            None => counts.push((tool, 1)),
        }
    }
    counts
        .iter()
        .map(|(name, n)| {
            if *n > 1 {
                format!("{name} ×{n}")
            } else {
                (*name).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Consecutive assistant messages are one reply, as Claude Code shows them.
/// A turn with no reply text but tool calls still gets its reply section.
fn body(exchanges: &[Exchange]) -> String {
    let mut out = String::new();
    for e in exchanges {
        if let Some(prompt) = &e.prompt {
            out.push_str(&format!(
                "\n## You · turn {}\n\n{}\n\n",
                e.number,
                prompt.trim()
            ));
        }
        if e.replies.is_empty() && e.tools.is_empty() {
            continue;
        }
        out.push_str("\n## Claude\n\n");
        for text in &e.replies {
            out.push_str(text.trim());
            out.push_str("\n\n");
        }
        if !e.tools.is_empty() {
            out.push_str(&format!("*Tools: {}*\n\n", tool_summary(&e.tools)));
        }
    }
    out
}

/// The whole session, or the turns `selection` keeps.
pub fn markdown(
    located: &Located,
    id: &str,
    live: &HashSet<String>,
    selection: Option<Selection>,
) -> Result<String> {
    let detail = show(located, id, live)?;
    let path = located.transcript(id);
    let map = Mapped::open(&path).map_err(|e| SessileError::io("read", &path, &e))?;
    let all = exchanges(turns(map.bytes()));
    let Some(selection) = selection else {
        return Ok(format!("{}{}", header(&detail, None), body(&all)));
    };
    let kept = select(all, selection);
    let shown = match (kept.first(), kept.last()) {
        (Some(a), Some(b)) => format!("{}-{} of {}", a.number, b.number, detail.row.prompts),
        _ => format!("none of {}", detail.row.prompts),
    };
    Ok(format!("{}{}", header(&detail, Some(&shown)), body(&kept)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_export_is_cut_inline_and_kept_whole_in_a_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let id = "0a1b2c3d-1111-4222-8333-444455556666";
        let short = document(dir.path(), id, "# t\n".into()).unwrap();
        assert!(!short.truncated && short.output_file.is_none());

        let long = "ą".repeat(INLINE_MAX_BYTES);
        let doc = document(dir.path(), id, long.clone()).unwrap();
        assert!(doc.truncated);
        assert!(doc.markdown.len() <= INLINE_MAX_BYTES && long.starts_with(&doc.markdown));
        let kept = fs::read_to_string(doc.output_file.unwrap()).unwrap();
        assert_eq!(kept, long);
    }

    #[test]
    fn replies_merge_and_tools_are_counted() {
        use crate::scan::Turn;
        let turns = vec![
            Turn::Prompt("hi".into()),
            Turn::Reply {
                text: Some("one".into()),
                tools: vec!["Bash".into()],
            },
            Turn::Reply {
                text: None,
                tools: vec!["Read".into(), "Bash".into()],
            },
            Turn::Reply {
                text: Some("two".into()),
                tools: vec![],
            },
            Turn::Prompt("bye".into()),
        ];
        assert_eq!(
            body(&exchanges(turns)),
            "\n## You · turn 1\n\nhi\n\n\n## Claude\n\none\n\ntwo\n\n*Tools: Bash ×2, Read*\n\n\n## You · turn 2\n\nbye\n\n"
        );
    }
}
