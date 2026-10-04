//! Reads transcripts without loading them into a parser: byte search finds the
//! lines worth parsing, serde parses only those.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;

use memchr::{memchr, memmem, memrchr};
use memmap2::Mmap;
use rayon::prelude::*;
use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

/// Characters kept per collected prompt (first/last prompts, snippets).
pub const PROMPT_KEEP: usize = 300;
/// Characters of the first prompt used as a fallback title.
pub const TITLE_KEEP: usize = 80;
const KEEP_PROMPTS: usize = 3;
/// Lines read from the end of a transcript when looking for branch/model.
const TAIL_LINES: usize = 3000;

const USER_NEEDLE: &[u8] = br#""type":"user""#;
const ASSISTANT_NEEDLE: &[u8] = br#""type":"assistant""#;
/// Both title entry kinds contain this; a line is a title entry only if it also starts with a prefix.
const TITLE_NEEDLE: &[u8] = br#"-title","#;
const CUSTOM_TITLE_PREFIX: &[u8] = br#"{"type":"custom-title""#;
const AI_TITLE_PREFIX: &[u8] = br#"{"type":"ai-title""#;

/// Read-only view of a transcript file.
pub enum Mapped {
    Empty,
    Map(Mmap),
}

impl Mapped {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() == 0 {
            return Ok(Self::Empty);
        }
        // SAFETY: the map is read-only and short-lived. Claude Code only
        // appends to transcripts, and a concurrent truncate is the same
        // hazard every mmap-based reader accepts.
        let map = unsafe { Mmap::map(&file)? };
        Ok(Self::Map(map))
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Empty => &[],
            Self::Map(m) => m,
        }
    }
}

// ---- lenient serde pieces --------------------------------------------------

/// A JSON value that is kept when it is a string and skipped otherwise.
#[derive(Default)]
struct MaybeStr<'a>(Option<Cow<'a, str>>);

impl MaybeStr<'_> {
    fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

struct MaybeStrVisitor;

impl<'de> Visitor<'de> for MaybeStrVisitor {
    type Value = MaybeStr<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Borrowed(v))))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Owned(v.to_owned()))))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Owned(v))))
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(MaybeStr(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(MaybeStr(None))
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for MaybeStr<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(MaybeStrVisitor)
    }
}

#[derive(Default, Deserialize)]
struct Block<'a> {
    #[serde(rename = "type", default, borrow)]
    kind: MaybeStr<'a>,
    #[serde(default, borrow)]
    text: MaybeStr<'a>,
    #[serde(default, borrow)]
    name: MaybeStr<'a>,
}

/// `message.content`: a plain string or an array of blocks.
enum Content<'a> {
    Text(Cow<'a, str>),
    Blocks(Vec<Block<'a>>),
    Other,
}

struct ContentVisitor;

impl<'de> Visitor<'de> for ContentVisitor {
    type Value = Content<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string or an array of blocks")
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
        Ok(Content::Text(Cow::Borrowed(v)))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Content::Text(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(Content::Text(Cow::Owned(v)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut blocks = Vec::new();
        while let Some(block) = seq.next_element::<Block<'de>>()? {
            blocks.push(block);
        }
        Ok(Content::Blocks(blocks))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Content::Other)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Content::Other)
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Content<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ContentVisitor)
    }
}

impl Content<'_> {
    fn has_tool_result(&self) -> bool {
        match self {
            Self::Blocks(blocks) => blocks
                .iter()
                .any(|b| b.kind.as_str() == Some("tool_result")),
            _ => false,
        }
    }

    fn tool_names(&self) -> Vec<String> {
        match self {
            Self::Blocks(blocks) => blocks
                .iter()
                .filter(|b| b.kind.as_str() == Some("tool_use"))
                .filter_map(|b| b.name.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The text of a string content, or the text blocks joined by newlines.
    /// `None` when there is no text block (tool results, images only).
    fn text(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Text(t) => Some(Cow::Borrowed(t)),
            Self::Other => None,
            Self::Blocks(blocks) => {
                let mut texts = blocks
                    .iter()
                    .filter(|b| b.kind.as_str() == Some("text"))
                    .filter_map(|b| b.text.as_str());
                let first = texts.next()?;
                match texts.next() {
                    None => Some(Cow::Borrowed(first)),
                    Some(second) => {
                        let mut joined = format!("{first}\n{second}");
                        for t in texts {
                            joined.push('\n');
                            joined.push_str(t);
                        }
                        Some(Cow::Owned(joined))
                    }
                }
            }
        }
    }
}

#[derive(Default, Deserialize)]
struct Message<'a> {
    #[serde(default, borrow)]
    role: MaybeStr<'a>,
    #[serde(default, borrow)]
    model: MaybeStr<'a>,
    #[serde(default, borrow)]
    content: Option<Content<'a>>,
}

#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(rename = "type", default, borrow)]
    kind: MaybeStr<'a>,
    #[serde(rename = "isMeta", default)]
    is_meta: Option<bool>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: Option<bool>,
    #[serde(rename = "isCompactSummary", default)]
    is_compact_summary: Option<bool>,
    #[serde(rename = "isVisibleInTranscriptOnly", default)]
    is_transcript_only: Option<bool>,
    #[serde(default, borrow)]
    message: Option<Message<'a>>,
}

impl Entry<'_> {
    fn is_synthetic(&self) -> bool {
        [
            self.is_meta,
            self.is_sidechain,
            self.is_compact_summary,
            self.is_transcript_only,
        ]
        .contains(&Some(true))
    }

    /// A content block that is neither text nor a tool result.
    fn has_attachment(&self) -> bool {
        let content = self.message.as_ref().and_then(|m| m.content.as_ref());
        matches!(content, Some(Content::Blocks(blocks)) if blocks
            .iter()
            .any(|b| !matches!(b.kind.as_str(), Some("text" | "tool_result"))))
    }
}

#[derive(Deserialize)]
struct TitleLine {
    #[serde(rename = "customTitle", default)]
    custom: Option<String>,
    #[serde(rename = "aiTitle", default)]
    ai: Option<String>,
}

#[derive(Deserialize)]
struct MetaLine<'a> {
    #[serde(default, borrow)]
    cwd: MaybeStr<'a>,
    #[serde(rename = "gitBranch", default, borrow)]
    git_branch: MaybeStr<'a>,
    #[serde(default, borrow)]
    version: MaybeStr<'a>,
    #[serde(default, borrow)]
    entrypoint: MaybeStr<'a>,
    #[serde(default, borrow)]
    timestamp: MaybeStr<'a>,
}

// ---- line iteration --------------------------------------------------------

/// Lines that contain `needle`, each yielded once, found with SIMD search
/// instead of splitting the whole file. A hit whose following bytes start with
/// one of `skip` is passed over without locating the line, which keeps
/// megabyte tool-result lines from being touched at all.
fn lines_containing<'a>(
    data: &'a [u8],
    needle: &[u8],
    skip: &'static [&'static [u8]],
) -> impl Iterator<Item = &'a [u8]> + use<'a> {
    let finder = memmem::Finder::new(needle).into_owned();
    let needle_len = needle.len();
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        loop {
            if pos >= data.len() {
                return None;
            }
            let hit = pos + finder.find(&data[pos..])?;
            let after = &data[hit + needle_len..];
            if skip.iter().any(|s| after.starts_with(s)) {
                pos = hit + needle_len;
                continue;
            }
            let start = memrchr(b'\n', &data[..hit]).map_or(0, |i| i + 1);
            let end = memchr(b'\n', &data[hit..]).map_or(data.len(), |i| hit + i);
            pos = end + 1;
            return Some(&data[start..end]);
        }
    })
}

/// Non-empty lines from the end of the buffer.
fn lines_rev(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut end = data.len();
    std::iter::from_fn(move || {
        while end > 0 {
            let start = memrchr(b'\n', &data[..end]).map_or(0, |i| i + 1);
            let line = &data[start..end];
            end = start.saturating_sub(1);
            if !line.is_empty() {
                return Some(line);
            }
        }
        None
    })
}

// ---- prompts ---------------------------------------------------------------

/// Text that is injected by Claude Code or other tools rather than typed by
/// the user. The one place that decides what is not a prompt.
fn is_noise(text: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "<command-name>",
        "<command-message>",
        "<command-args>",
        "<local-command-",
        "<system-reminder>",
        "<task-notification>",
        "<task-id",
        "<bash-input>",
        "<bash-stdout>",
        "<bash-stderr>",
        "<teammate-message",
        "<user-prompt-submit-hook>",
        "Caveat:",
        "[Request interrupted",
        "Another Claude session sent",
        "This session is being continued",
    ];
    let head = text.trim_start();
    PREFIXES.iter().any(|p| head.starts_with(p))
}

/// What follows `"type":"user"` on a message whose first content block is a
/// tool result. Such lines are the bulk of a transcript and can be megabytes,
/// so they are skipped before the line is even located. Other key orders (older
/// versions) fall through to the parser, which also rejects them.
const TOOL_RESULT_USER: &[&[u8]] = &[
    br#","message":{"role":"user","content":[{"tool_use_id""#,
    br#","message":{"role":"user","content":[{"type":"tool_result""#,
];

/// Calls `f` with the text of a user prompt line, if the line is one.
fn with_user_prompt(line: &[u8], f: impl FnOnce(&str)) {
    let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
        return;
    };
    if entry.kind.as_str() != Some("user") || entry.is_synthetic() {
        return;
    }
    let Some(message) = &entry.message else {
        return;
    };
    if message.role.as_str() != Some("user") {
        return;
    }
    let Some(content) = &message.content else {
        return;
    };
    // A message that carries a tool result is the harness answering the model,
    // even when it has a text block next to it ("Tool loaded.").
    if content.has_tool_result() {
        return;
    }
    let Some(text) = content.text() else {
        return;
    };
    if text.trim().is_empty() || is_noise(&text) {
        return;
    }
    f(&text);
}

/// Calls `f` with the full text of every user prompt, in file order.
pub fn for_each_prompt(data: &[u8], mut f: impl FnMut(&str)) {
    for line in lines_containing(data, USER_NEEDLE, TOOL_RESULT_USER) {
        with_user_prompt(line, &mut f);
    }
}

/// Calls `f` with the text blocks of every assistant message.
pub fn for_each_assistant_text(data: &[u8], mut f: impl FnMut(&str)) {
    for line in lines_containing(data, ASSISTANT_NEEDLE, &[]) {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            continue;
        };
        if entry.kind.as_str() != Some("assistant") || entry.is_synthetic() {
            continue;
        }
        if let Some(text) = entry
            .message
            .as_ref()
            .and_then(|m| m.content.as_ref())
            .and_then(Content::text)
        {
            f(&text);
        }
    }
}

/// Whether the transcript holds anything worth keeping besides counted
/// prompts: an assistant message, or a user message with an attachment (an
/// image, a file). A session started by a slash command, a screenshot or `!`
/// input has no counted prompt and still a conversation. A line that does not
/// parse counts as conversation: what cannot be read is not known to be empty.
pub fn has_conversation(data: &[u8]) -> bool {
    let assistant = lines_containing(data, ASSISTANT_NEEDLE, &[]).any(|line| {
        serde_json::from_slice::<Entry>(line).map_or(true, |e| {
            e.kind.as_str() == Some("assistant") && !e.is_synthetic()
        })
    });
    assistant
        || lines_containing(data, USER_NEEDLE, TOOL_RESULT_USER).any(|line| {
            serde_json::from_slice::<Entry>(line).map_or(true, |e| {
                e.kind.as_str() == Some("user") && !e.is_synthetic() && e.has_attachment()
            })
        })
}

/// One step of the conversation as a person reads it.
pub enum Turn {
    Prompt(String),
    Reply {
        text: Option<String>,
        tools: Vec<String>,
    },
}

/// Prompts and assistant messages in file order. Tool results and injected
/// text are left out, as in every other view.
pub fn turns(data: &[u8]) -> Vec<Turn> {
    let at = |line: &[u8]| line.as_ptr().addr() - data.as_ptr().addr();
    let mut found: Vec<(usize, Turn)> = Vec::new();
    for line in lines_containing(data, USER_NEEDLE, TOOL_RESULT_USER) {
        with_user_prompt(line, |text| {
            found.push((at(line), Turn::Prompt(text.to_string())))
        });
    }
    for line in lines_containing(data, ASSISTANT_NEEDLE, &[]) {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            continue;
        };
        if entry.kind.as_str() != Some("assistant") || entry.is_synthetic() {
            continue;
        }
        let Some(content) = entry.message.as_ref().and_then(|m| m.content.as_ref()) else {
            continue;
        };
        let reply = Turn::Reply {
            text: content.text().map(Cow::into_owned),
            tools: content.tool_names(),
        };
        found.push((at(line), reply));
    }
    found.sort_by_key(|(offset, _)| *offset);
    found.into_iter().map(|(_, turn)| turn).collect()
}

/// Whitespace collapsed, cut to `max` characters with an ellipsis.
pub fn clean(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut chars = 0usize;
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
            chars += 1;
        }
        out.push_str(word);
        chars += word.chars().count();
        if chars > max {
            break;
        }
    }
    truncate_chars(&out, max)
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
    }
}

#[derive(Default)]
pub struct PromptStats {
    pub count: usize,
    pub first: Vec<String>,
    pub last: VecDeque<String>,
}

impl PromptStats {
    pub fn push(&mut self, text: &str) {
        self.count += 1;
        let cleaned = clean(text, PROMPT_KEEP);
        if self.first.len() < KEEP_PROMPTS {
            self.first.push(cleaned.clone());
        }
        if self.last.len() == KEEP_PROMPTS {
            self.last.pop_front();
        }
        self.last.push_back(cleaned);
    }

    /// Appends the stats of a later part of the same transcript.
    fn merge(&mut self, later: Self) {
        self.count += later.count;
        let room = KEEP_PROMPTS.saturating_sub(self.first.len());
        self.first.extend(later.first.into_iter().take(room));
        self.last.extend(later.last);
        while self.last.len() > KEEP_PROMPTS {
            self.last.pop_front();
        }
    }
}

pub fn prompt_stats(data: &[u8]) -> PromptStats {
    let mut stats = PromptStats::default();
    for_each_prompt(data, |t| stats.push(t));
    stats
}

// ---- titles and metadata ---------------------------------------------------

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Titles {
    pub custom: Option<String>,
    pub ai: Option<String>,
}

/// The last title entry of each kind. Title entries start their line, so a
/// match preceded by anything else (a pasted copy inside a message) is skipped.
pub fn find_titles(data: &[u8]) -> Titles {
    TitleLines::scan(data).resolve()
}

/// Title entry lines of one chunk, in file order.
#[derive(Default)]
struct TitleLines<'a> {
    custom: Vec<&'a [u8]>,
    ai: Vec<&'a [u8]>,
}

impl<'a> TitleLines<'a> {
    /// One forward pass with a shared, rare needle; backward search is several times slower.
    fn scan(data: &'a [u8]) -> Self {
        let finder = memmem::Finder::new(TITLE_NEEDLE);
        let mut lines = Self::default();
        for hit in finder.find_iter(data) {
            let start = memrchr(b'\n', &data[..hit]).map_or(0, |i| i + 1);
            let end = memchr(b'\n', &data[hit..]).map_or(data.len(), |i| hit + i);
            let line = &data[start..end];
            if line.starts_with(CUSTOM_TITLE_PREFIX) {
                lines.custom.push(line);
            } else if line.starts_with(AI_TITLE_PREFIX) {
                lines.ai.push(line);
            }
        }
        lines
    }

    fn append(&mut self, mut other: Self) {
        self.custom.append(&mut other.custom);
        self.ai.append(&mut other.ai);
    }

    fn resolve(&self) -> Titles {
        Titles {
            custom: last_title(&self.custom, |t| t.custom),
            ai: last_title(&self.ai, |t| t.ai),
        }
    }
}

/// Transcripts at least this large are scanned in parallel chunks.
const CHUNK_BYTES: usize = 1 << 20;

/// Splits at line boundaries into chunks of roughly `CHUNK_BYTES`.
fn chunks(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::with_capacity(data.len() / CHUNK_BYTES + 1);
    let mut start = 0;
    while start < data.len() {
        let target = (start + CHUNK_BYTES).min(data.len());
        let end = if target == data.len() {
            target
        } else {
            memchr(b'\n', &data[target..]).map_or(data.len(), |i| target + i + 1)
        };
        out.push(&data[start..end]);
        start = end;
    }
    out
}

/// Titles and prompt stats of a whole transcript, in one parallel pass over
/// line-aligned chunks. Chunk results merge in file order.
pub fn scan_transcript(data: &[u8]) -> (Titles, PromptStats) {
    let parts: Vec<(TitleLines<'_>, PromptStats)> = chunks(data)
        .par_iter()
        .map(|chunk| (TitleLines::scan(chunk), prompt_stats(chunk)))
        .collect();
    let mut titles = TitleLines::default();
    let mut stats = PromptStats::default();
    for (t, s) in parts {
        titles.append(t);
        stats.merge(s);
    }
    (titles.resolve(), stats)
}

/// Title of the last entry that parses; a torn last line (live writer) falls
/// back to the previous entry. An empty title counts as cleared.
fn last_title(lines: &[&[u8]], pick: impl Fn(TitleLine) -> Option<String>) -> Option<String> {
    let line = lines
        .iter()
        .rev()
        .find_map(|l| serde_json::from_slice::<TitleLine>(l).ok())?;
    pick(line)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[derive(Default, Debug)]
pub struct Meta {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub model: Option<String>,
    pub interactive: Option<bool>,
    pub created_at: Option<String>,
}

/// What the first entry with a `cwd` records about how the session started.
#[derive(Debug, Default)]
pub struct Head {
    pub cwd: Option<String>,
    pub version: Option<String>,
    pub interactive: Option<bool>,
    pub created_at: Option<String>,
}

/// Claude Code writes `2026-09-19T18:50:40.078Z`, the rows' own format; any
/// other shape is not passed off as one.
fn utc_ms(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    let shaped = b.len() == 24
        && b[4] == b'-'
        && b[10] == b'T'
        && b[19] == b'.'
        && b[23] == b'Z'
        && b.iter()
            .enumerate()
            .all(|(i, c)| [4, 7, 10, 13, 16, 19, 23].contains(&i) || c.is_ascii_digit());
    if !shaped {
        return None;
    }
    // Digits in the right places are not yet a time: month 99 is not one.
    let two = |at: usize| u32::from(b[at] - b'0') * 10 + u32::from(b[at + 1] - b'0');
    let in_range = (1..=12).contains(&two(5))
        && (1..=31).contains(&two(8))
        && two(11) <= 23
        && two(14) <= 59
        && two(17) <= 60;
    in_range.then(|| raw.to_string())
}

/// The first `cwd` in the head, with the version and entrypoint recorded
/// next to it. `sdk-*` entrypoints (`claude -p`, the Agent SDK) are not
/// interactive and any other value is. Claude Code before 2.1.78 wrote no
/// entrypoint, and nothing else in those transcripts tells the two apart, so
/// they stay unknown.
pub fn find_head(data: &[u8]) -> Head {
    for line in data.split(|b| *b == b'\n').take(200) {
        if memmem::find(line, br#""cwd":""#).is_none() {
            continue;
        }
        if let Ok(m) = serde_json::from_slice::<MetaLine>(line)
            && let Some(cwd) = m.cwd.as_str().filter(|c| !c.is_empty())
        {
            return Head {
                cwd: Some(cwd.to_string()),
                version: m.version.as_str().map(String::from),
                interactive: m.entrypoint.as_str().map(|e| !e.starts_with("sdk-")),
                created_at: m.timestamp.as_str().and_then(utc_ms),
            };
        }
    }
    Head::default()
}

/// cwd from the head; branch, version and model from the tail.
pub fn find_meta(data: &[u8]) -> Meta {
    let head = find_head(data);
    let mut meta = Meta {
        cwd: head.cwd,
        version: head.version,
        interactive: head.interactive,
        created_at: head.created_at,
        ..Meta::default()
    };
    let mut branch_seen = false;
    for line in lines_rev(data).take(TAIL_LINES) {
        if !branch_seen
            && memmem::find(line, br#""gitBranch":""#).is_some()
            && let Ok(m) = serde_json::from_slice::<MetaLine>(line)
        {
            branch_seen = true;
            meta.git_branch = m
                .git_branch
                .as_str()
                .filter(|b| !b.is_empty())
                .map(String::from);
            if let Some(v) = m.version.as_str().filter(|v| !v.is_empty()) {
                meta.version = Some(v.to_string());
            }
        }
        if meta.model.is_none() && memmem::find(line, ASSISTANT_NEEDLE).is_some() {
            meta.model = assistant_model(line);
        }
        if branch_seen && meta.model.is_some() {
            break;
        }
    }
    meta
}

fn assistant_model(line: &[u8]) -> Option<String> {
    let entry = serde_json::from_slice::<Entry>(line).ok()?;
    if entry.kind.as_str() != Some("assistant") {
        return None;
    }
    entry
        .message?
        .model
        .as_str()
        .filter(|m| !m.is_empty() && *m != "<synthetic>")
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> String {
        format!(r#"{{"type":"user","message":{{"role":"user","content":{content}}}}}"#)
    }

    fn prompts(lines: &[String]) -> Vec<String> {
        let data = lines.join("\n");
        let mut out = Vec::new();
        for_each_prompt(data.as_bytes(), |t| out.push(t.to_string()));
        out
    }

    #[test]
    fn counts_text_prompts_and_skips_noise() {
        let lines = vec![
            user(r#""hello there""#),
            user(r#"[{"type":"tool_result","tool_use_id":"x","content":"ok"}]"#),
            user(r#"[{"type":"text","text":"second"},{"type":"text","text":"part"}]"#),
            user(r#""<command-name>/compact</command-name>""#),
            user(r#""<local-command-stdout>x</local-command-stdout>""#),
            user(r#""<system-reminder>x</system-reminder>""#),
            user(r#""Caveat: messages below""#),
            user(r#""   ""#),
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta"}}"#.into(),
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"sum"}}"#
                .into(),
            r#"{"type":"assistant","message":{"role":"assistant","content":"hi"}}"#.into(),
            "not json but has \"type\":\"user\"".into(),
        ];
        assert_eq!(prompts(&lines), vec!["hello there", "second\npart"]);
    }

    #[test]
    fn title_entries_must_start_their_line() {
        let data = concat!(
            r#"{"type":"ai-title","aiTitle":"First","sessionId":"s"}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"Second","sessionId":"s"}"#,
            "\n",
            r#"{"type":"x","payload":{"type":"custom-title","customTitle":"fake"}}"#,
            "\n",
            r#"{"type":"custom-title","customTitle":"Mine","sessionId":"s"}"#,
            "\n",
            r#"{"type":"custom-title","customTitle":"Mine v2","sessionId":"s"}"#,
            "\n",
            r#"{"type":"custom-title","customTi"#,
        );
        let titles = find_titles(data.as_bytes());
        assert_eq!(titles.custom.as_deref(), Some("Mine v2"));
        assert_eq!(titles.ai.as_deref(), Some("Second"));
    }

    #[test]
    fn clean_collapses_and_truncates() {
        assert_eq!(clean("a  b\n\nc", 10), "a b c");
        assert_eq!(clean("abcdefghij", 4), "abcd…");
    }
}
