//! A session as numbered turns: turn N is the Nth prompt a person typed (the
//! prompts a row counts) with the replies after it. `export` and `excerpts`
//! number them the same way, so a turn found by one can be read with the other.

use serde::Serialize;

use crate::scan::Turn;
use crate::search::{Query, excerpt};

/// One prompt and the replies after it. Turn 0 holds replies that come before
/// the first prompt, and only exists when there are some.
#[derive(Debug, Default)]
pub struct Exchange {
    pub number: usize,
    pub prompt: Option<String>,
    pub replies: Vec<String>,
    pub tools: Vec<String>,
}

pub fn exchanges(turns: Vec<Turn>) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    let mut prompts = 0;
    for turn in turns {
        match turn {
            Turn::Prompt(text) => {
                prompts += 1;
                out.push(Exchange {
                    number: prompts,
                    prompt: Some(text),
                    ..Exchange::default()
                });
            }
            Turn::Reply { text, tools } => {
                if out.is_empty() {
                    out.push(Exchange::default());
                }
                let last = out.last_mut().expect("pushed above");
                last.replies.extend(text.filter(|t| !t.trim().is_empty()));
                last.tools.extend(tools);
            }
        }
    }
    out
}

/// Which turns to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// From the first number to the second, or to the end.
    Range(usize, Option<usize>),
    /// The last N prompts and their replies.
    Last(usize),
}

/// `12`, `12-15` or `12-`.
pub fn parse_range(raw: &str) -> Result<Selection, String> {
    let number = |s: &str| {
        s.trim()
            .parse::<usize>()
            .map_err(|_| "expected N, A-B or A- with whole numbers, such as 12-15".to_string())
    };
    let (from, to) = match raw.split_once('-') {
        None => {
            let n = number(raw)?;
            (n, Some(n))
        }
        Some((from, "")) => (number(from)?, None),
        Some((from, to)) => (number(from)?, Some(number(to)?)),
    };
    if to.is_some_and(|to| to < from) {
        return Err(format!("the range ends before it starts: {raw}"));
    }
    Ok(Selection::Range(from, to))
}

pub fn select(all: Vec<Exchange>, selection: Selection) -> Vec<Exchange> {
    let prompts = all.iter().map(|e| e.number).max().unwrap_or(0);
    let (from, to) = match selection {
        Selection::Range(from, to) => (from, to.unwrap_or(usize::MAX)),
        // Asking for every turn includes turn 0.
        Selection::Last(n) if n >= prompts => (0, usize::MAX),
        Selection::Last(n) => (prompts - n + 1, usize::MAX),
    };
    all.into_iter()
        .filter(|e| (from..=to).contains(&e.number))
        .collect()
}

#[derive(Serialize, Debug)]
pub struct Excerpt {
    pub turn: usize,
    pub role: &'static str,
    pub snippet: String,
}

/// Every prompt and every reply that holds all the query's words, in
/// conversation order.
pub fn excerpts(all: &[Exchange], query: &Query) -> Vec<Excerpt> {
    let mut out = Vec::new();
    for e in all {
        let hit = |role, text: &str| {
            excerpt(query, text).map(|snippet| Excerpt {
                turn: e.number,
                role,
                snippet,
            })
        };
        out.extend(e.prompt.as_deref().and_then(|p| hit("prompt", p)));
        out.extend(hit("assistant", &e.replies.join("\n\n")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(text: &str) -> Turn {
        Turn::Reply {
            text: Some(text.into()),
            tools: vec![],
        }
    }

    fn sample() -> Vec<Exchange> {
        exchanges(vec![
            reply("before anything"),
            Turn::Prompt("deploy the router".into()),
            reply("deployed"),
            reply("router is up"),
            Turn::Prompt("now the docs".into()),
            Turn::Prompt("and the router docs".into()),
            reply("done"),
        ])
    }

    #[test]
    fn turns_are_numbered_by_prompt() {
        let all = sample();
        let numbers: Vec<usize> = all.iter().map(|e| e.number).collect();
        assert_eq!(numbers, vec![0, 1, 2, 3]);
        assert_eq!(all[1].replies, vec!["deployed", "router is up"]);
        assert!(all[2].replies.is_empty());
    }

    #[test]
    fn ranges_parse_and_select() {
        assert_eq!(parse_range("2"), Ok(Selection::Range(2, Some(2))));
        assert_eq!(parse_range("2-3"), Ok(Selection::Range(2, Some(3))));
        assert_eq!(parse_range("2-"), Ok(Selection::Range(2, None)));
        assert!(parse_range("3-2").is_err());
        assert!(parse_range("-2").is_err());
        assert!(parse_range("x").is_err());
        let numbers = |s| {
            select(sample(), s)
                .iter()
                .map(|e| e.number)
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(Selection::Range(2, None)), vec![2, 3]);
        assert_eq!(numbers(Selection::Last(2)), vec![2, 3]);
        assert_eq!(numbers(Selection::Last(9)), vec![0, 1, 2, 3]);
        assert_eq!(numbers(Selection::Last(0)), Vec::<usize>::new());
    }

    #[test]
    fn excerpts_name_the_turn_and_the_side() {
        let query = Query::new("router").unwrap();
        let found: Vec<(usize, &str)> = excerpts(&sample(), &query)
            .iter()
            .map(|e| (e.turn, e.role))
            .collect();
        assert_eq!(found, vec![(1, "prompt"), (1, "assistant"), (3, "prompt")]);
    }
}
