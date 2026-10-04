//! Output formats, the one place stdout is written, terminal-safe text and
//! RFC 3339 timestamps.

use std::borrow::Cow;
use std::io::{self, IsTerminal, Write};
use std::process;

use serde::Serialize;

use crate::error::{Kind, SessileError};

/// What stdout carries for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Human-readable table or lines, the default at a terminal.
    Text,
    /// One JSON document, the default when stdout is not a terminal.
    Json,
    /// One item per line (`list`, `search`).
    Plain,
    /// The export's Markdown document.
    Markdown,
}

/// The explicit flag wins; otherwise the command's native document format,
/// or text at a terminal and JSON elsewhere.
pub fn resolve(json: bool, plain: bool, native_markdown: bool) -> Format {
    if json {
        Format::Json
    } else if plain {
        Format::Plain
    } else if native_markdown {
        Format::Markdown
    } else if io::stdout().is_terminal() {
        Format::Text
    } else {
        Format::Json
    }
}

/// The error object goes to stderr when the output is for a program (`--json`,
/// or stdout is not a terminal) or stderr is not a person's terminal.
pub fn structured_errors(json: bool) -> bool {
    json || !io::stdout().is_terminal() || !io::stderr().is_terminal()
}

/// Writes the whole result. A reader that closed the pipe ends the process
/// quietly with exit 0: there is nobody left to tell.
pub fn write_stdout(text: &str) {
    let mut out = io::stdout().lock();
    let result = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    if let Err(e) = result {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        let error = SessileError::new(Kind::IoError, format!("cannot write the result: {e}"));
        if structured_errors(false) {
            eprintln!("{}", error.to_json());
        } else {
            eprintln!("Error: {}", error.message);
        }
        process::exit(1);
    }
}

pub fn write_json(value: &impl Serialize) {
    let mut text = serde_json::to_string(value).expect("output values always encode");
    text.push('\n');
    write_stdout(&text);
}

/// ESC and the C1 controls shown as `\u{1b}` text, so a transcript cannot
/// move the cursor or recolour the terminal it is printed on.
pub fn escape_controls(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_terminal_control) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if is_terminal_control(c) {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

fn is_terminal_control(c: char) -> bool {
    c == '\u{1b}' || ('\u{80}'..='\u{9f}').contains(&c)
}

/// One `--plain` field: control sequences escaped as for text, then `\`,
/// tab, LF and CR written as `\\`, `\t`, `\n`, `\r`, so a value never splits
/// a line or a column.
pub fn plain_field(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if is_terminal_control(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Unix milliseconds as RFC 3339 UTC with millisecond precision.
pub fn rfc3339_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_utc_with_milliseconds() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_791_036_643_207), "2026-10-03T14:10:43.207Z");
        assert_eq!(rfc3339_ms(4_107_542_399_999), "2100-02-28T23:59:59.999Z");
    }

    #[test]
    fn terminal_controls_become_visible_text() {
        assert_eq!(escape_controls("plain"), "plain");
        assert_eq!(
            escape_controls("a\u{1b}[2Jb\u{9b}c"),
            "a\\u{1b}[2Jb\\u{9b}c"
        );
        assert_eq!(escape_controls("tab\tstays"), "tab\tstays");
    }

    #[test]
    fn plain_fields_keep_one_line() {
        assert_eq!(plain_field("a\tb\nc\\d\u{1b}"), "a\\tb\\nc\\\\d\\u{1b}");
    }
}
