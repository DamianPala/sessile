//! Ranked search over one project: title and prompts first, assistant text last.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use clap::ValueEnum;
use rayon::prelude::*;
use serde::Serialize;

use crate::error::{Result, SessileError};
use crate::scan::{
    Mapped, PromptStats, clean, find_head, find_titles, for_each_assistant_text, for_each_prompt,
};
use crate::session::{Row, Scan, Scanned, Unreadable, build_row, split, transcripts_of};

const WEIGHT_TITLE: u32 = 3;
const WEIGHT_PROMPT: u32 = 2;
const WEIGHT_ASSISTANT: u32 = 1;
const SNIPPET_CHARS: usize = 100;
const SNIPPET_LEAD_BYTES: usize = 30;
const LEXICAL_PER_CHAR: u32 = 20;
const WORD_START_BONUS: u32 = 10;
/// Extra score per additional assistant block that matches, capped.
const REPEAT_BONUS: u32 = 5;
const REPEAT_CAP: u32 = 10;
/// Typo tolerance per query word: words this short must match as written,
/// longer ones may be this many edits (Damerau) from a title word.
const TYPO_MIN_CHARS: usize = 5;
const TYPO_ONE_EDIT_MAX_CHARS: usize = 6;
/// Score lost per edit, so closer typo matches rank higher.
const TYPO_EDIT_PENALTY: u32 = 10;

/// Where a query may match.
#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchField {
    Title,
    Prompt,
    Assistant,
}

pub const ALL_FIELDS: [MatchField; 3] =
    [MatchField::Title, MatchField::Prompt, MatchField::Assistant];

#[derive(Serialize, Debug)]
pub struct Hit {
    #[serde(flatten)]
    pub row: Row,
    pub score: u32,
    pub matched_in: &'static str,
    /// The best match is a typo-tolerant title match, not the query's words as written.
    pub fuzzy: bool,
    pub snippet: String,
}

pub struct Query {
    tokens: Vec<String>,
    /// Every fuzzy score stays below the lowest exact score of any field, so
    /// an exact hit always ranks first.
    fuzzy_cap: u32,
}

impl Query {
    pub fn new(raw: &str) -> Option<Self> {
        let tokens: Vec<String> = raw.split_whitespace().map(str::to_lowercase).collect();
        if tokens.is_empty() {
            return None;
        }
        let chars: usize = tokens.iter().map(|t| t.chars().count()).sum();
        let chars = u32::try_from(chars).unwrap_or(u32::MAX);
        Some(Self {
            fuzzy_cap: LEXICAL_PER_CHAR
                .saturating_mul(chars)
                .saturating_mul(WEIGHT_ASSISTANT)
                .saturating_sub(1),
            tokens,
        })
    }
}

struct Candidate {
    score: u32,
    is_fuzzy: bool,
    snippet: String,
}

struct Scorer<'a> {
    query: &'a Query,
    fields: &'a [MatchField],
}

impl Scorer<'_> {
    /// Every query token appears as a case-insensitive substring.
    fn lexical(&self, text: &str) -> Option<Candidate> {
        let lower = text.to_lowercase();
        let mut score = 0u32;
        let mut first = usize::MAX;
        for token in &self.query.tokens {
            let pos = lower.find(token.as_str())?;
            let chars = u32::try_from(token.chars().count()).unwrap_or(u32::MAX);
            score = score.saturating_add(LEXICAL_PER_CHAR.saturating_mul(chars));
            if at_word_start(&lower, pos) {
                score += WORD_START_BONUS;
            }
            first = first.min(pos);
        }
        // Lowercasing can change byte offsets for rare characters; then cut from the lowered text.
        let source = if lower.len() == text.len() {
            text
        } else {
            &lower
        };
        Some(Candidate {
            score,
            is_fuzzy: false,
            snippet: snippet_around(source, first),
        })
    }

    /// Every query word as written, or within a few edits of a word of the
    /// title. The score is the edits lost from the fuzzy cap; `offer` keeps
    /// it under every exact score.
    fn typo(&self, text: &str) -> Option<Candidate> {
        let lower = text.to_lowercase();
        let words: Vec<Vec<char>> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().collect())
            .collect();
        let mut edits = 0u32;
        for token in &self.query.tokens {
            if lower.contains(token.as_str()) {
                continue;
            }
            let token: Vec<char> = token.chars().collect();
            let allowed = match token.len() {
                n if n < TYPO_MIN_CHARS => return None,
                n if n <= TYPO_ONE_EDIT_MAX_CHARS => 1,
                _ => 2,
            };
            let nearest = words.iter().map(|w| edit_distance(&token, w)).min()?;
            if nearest > allowed {
                return None;
            }
            edits += u32::try_from(nearest).unwrap_or(u32::MAX);
        }
        Some(Candidate {
            score: self
                .query
                .fuzzy_cap
                .saturating_sub(TYPO_EDIT_PENALTY.saturating_mul(edits)),
            is_fuzzy: true,
            snippet: clean(text, SNIPPET_CHARS),
        })
    }

    /// The words as written, else a typo match. Only the title is matched
    /// with typos: in long text near-misses of any word turn up everywhere.
    fn title(&self, text: &str) -> Option<Candidate> {
        self.lexical(text).or_else(|| self.typo(text))
    }
}

/// A snippet around the first word of `query` when `text` holds them all as
/// written.
pub fn excerpt(query: &Query, text: &str) -> Option<String> {
    Scorer { query, fields: &[] }
        .lexical(text)
        .map(|c| c.snippet)
}

/// Optimal string alignment distance: insertions, deletions, substitutions
/// and swaps of two neighbours, each one edit.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut rows = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = d;
        }
    }
    rows[a.len()][b.len()]
}

fn at_word_start(lower: &str, pos: usize) -> bool {
    !lower[..pos]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

fn snippet_around(text: &str, pos: usize) -> String {
    let mut start = pos.saturating_sub(SNIPPET_LEAD_BYTES);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let body = clean(&text[start..], SNIPPET_CHARS);
    if start > 0 {
        format!("…{body}")
    } else {
        body
    }
}

struct Best {
    score: u32,
    field: &'static str,
    is_fuzzy: bool,
    snippet: String,
}

fn offer(
    best: &mut Option<Best>,
    (weight, fuzzy_cap): (u32, u32),
    field: &'static str,
    c: Candidate,
) {
    let score = if c.is_fuzzy {
        c.score.min(fuzzy_cap)
    } else {
        c.score.saturating_mul(weight)
    };
    if best.as_ref().is_none_or(|b| score > b.score) {
        *best = Some(Best {
            score,
            field,
            is_fuzzy: c.is_fuzzy,
            snippet: c.snippet,
        });
    }
}

fn search_file(path: &Path, live: &HashSet<String>, scorer: &Scorer) -> io::Result<Option<Hit>> {
    let md = fs::metadata(path)?;
    let map = Mapped::open(path)?;
    Ok(score_file(path, &md, map.bytes(), live, scorer))
}

fn score_file(
    path: &Path,
    md: &fs::Metadata,
    data: &[u8],
    live: &HashSet<String>,
    scorer: &Scorer,
) -> Option<Hit> {
    let fields = scorer.fields;
    let cap = scorer.query.fuzzy_cap;
    let titles = find_titles(data);
    let mut best: Option<Best> = None;
    // Prompts are always read: the row's prompt count and first prompt need them.
    let in_prompts = fields.contains(&MatchField::Prompt);
    let mut stats = PromptStats::default();
    for_each_prompt(data, |text| {
        stats.push(text);
        if in_prompts && let Some(c) = scorer.lexical(text) {
            offer(&mut best, (WEIGHT_PROMPT, cap), "prompt", c);
        }
    });
    // Only the title the row shows: an AI title behind a custom one would
    // make a hit nobody can see the reason for. A title made from the first
    // prompt is matched as that prompt.
    let shown = titles.custom.as_ref().or(titles.ai.as_ref());
    if fields.contains(&MatchField::Title)
        && let Some(c) = shown.and_then(|t| scorer.title(t))
    {
        offer(&mut best, (WEIGHT_TITLE, cap), "title", c);
    }
    if fields.contains(&MatchField::Assistant) {
        score_assistant(data, scorer, &mut best);
    }
    let best = best?;
    let head = find_head(data);
    let scanned = Scanned {
        titles: &titles,
        stats: &stats,
        cwd: head.cwd,
        interactive: head.interactive,
        created_at: head.created_at,
    };
    let row = build_row(path, md, scanned, live)?;
    Some(Hit {
        row,
        score: best.score,
        matched_in: best.field,
        fuzzy: best.is_fuzzy,
        snippet: best.snippet,
    })
}

fn score_assistant(data: &[u8], scorer: &Scorer, best: &mut Option<Best>) {
    let (mut top, mut hits): (Option<Candidate>, u32) = (None, 0);
    for_each_assistant_text(data, |text| {
        if let Some(c) = scorer.lexical(text) {
            hits += 1;
            if top.as_ref().is_none_or(|t| c.score > t.score) {
                top = Some(c);
            }
        }
    });
    if let Some(mut c) = top {
        c.score += REPEAT_BONUS * (hits - 1).min(REPEAT_CAP);
        offer(
            best,
            (WEIGHT_ASSISTANT, scorer.query.fuzzy_cap),
            "assistant",
            c,
        );
    }
}

/// Every hit for `raw` in the given projects and fields, best first, and the
/// transcripts that could not be read.
pub fn search(
    config: &Path,
    slugs: &[String],
    archived: bool,
    raw: &str,
    fields: &[MatchField],
    live: &HashSet<String>,
) -> Result<(Vec<Hit>, Vec<Unreadable>)> {
    let query = Query::new(raw).ok_or_else(|| {
        SessileError::usage("empty search query").with_hint("Pass at least one word to search for")
    })?;
    let scorer = Scorer {
        query: &query,
        fields,
    };
    let scans: Vec<Scan<Hit>> = transcripts_of(config, slugs, archived)
        .par_iter()
        .map(|path| Scan::from_io(path, search_file(path, live, &scorer)))
        .collect();
    let (mut hits, unreadable) = split(scans);
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.row.updated_ms.cmp(&a.row.updated_ms))
            .then_with(|| a.row.id.cmp(&b.row.id))
    });
    Ok((hits, unreadable))
}
