//! Argument parsing, command dispatch and output rendering.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;

use crate::error::{EXIT_USAGE, Kind, Result, SessileError};
use crate::export::{document, markdown};
use crate::interrupt;
use crate::introspect;
use crate::live::live_ids;
use crate::ops::{self, Confirm};
use crate::output::{
    Format, escape_controls, plain_field, resolve, structured_errors, write_json, write_stdout,
};
use crate::paths::{config_dir, expand_home, locate, project_slug, project_slugs};
use crate::pins;
use crate::scan::{Mapped, turns};
use crate::search::{ALL_FIELDS, MatchField, Query, search};
use crate::session::{Row, Unreadable, list_projects, show};
use crate::turns::{Excerpt, Selection, excerpts, exchanges, parse_range};

pub const DEFAULT_LIMIT: usize = 50;

/// Set by Claude Code for the commands it runs: the default of --current.
const CURRENT_SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";

const AFTER_HELP: &str = "\
Output: a table at a terminal, JSON when stdout is not a terminal. --json forces JSON; \
errors then come as one JSON object on the last line of stderr, as they do whenever \
stderr is not a terminal.
Command contracts as JSON: sessile schema [COMMAND]
Help for one command: sessile <COMMAND> --help
JSON shape: list and search print {items, has_more, partial}, one row per item (id, title, \
created_at, updated_at, prompts, cwd, ...); get prints one row with more fields.
Finding a session: sessile search WORDS --all --plain gives one id and title per line; \
--json rows carry matched_in (title, prompt or assistant) and score, and --in title,prompt \
skips sessions that only mention the words in replies. list and search return 50 sessions \
by default (--limit) and hide sessions started by claude -p or the Agent SDK \
(--include-headless shows them); --min-prompts 3 drops one-shot runs; then sessile get ID \
or sessile export ID.
Inside one session: sessile excerpts ID WORDS gives the turns where the words come up; \
sessile export ID --turns 12-15 (or --last-turns 3) prints just those turns.
Environment: CLAUDE_CONFIG_DIR is the Claude Code config directory \
(default: .claude in the home directory).
Exit codes: 0 success, 1 failure, 2 usage error, 130 interrupted (Ctrl-C).";

#[derive(Parser)]
#[command(
    name = "sessile",
    version,
    about = "Manage local Claude Code sessions: list, search, rename, archive, export and delete them",
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print the result as JSON (the default when stdout is not a terminal)
    #[arg(long, global = true)]
    json: bool,
    /// Directory the sessions were started in; default: the current directory. Selects the project of list, search and delete-empty; for commands that take an id it limits the lookup to that project (without it every project is searched)
    #[arg(long, global = true, value_name = "DIR")]
    project: Option<String>,
    /// Id of the session this process runs in: rename, archive, restore, delete and delete-empty refuse it (pin and unpin do not), and list, get and search report it live and current. Default: CLAUDE_CODE_SESSION_ID, which Claude Code sets for the commands it runs
    #[arg(long, global = true, value_name = "ID", value_parser = lower)]
    current: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// Which sessions a listing reads, and how much of it to return.
#[derive(Args)]
pub struct Listing {
    /// Read archived sessions instead of live ones
    #[arg(long)]
    archived: bool,
    /// Read every project instead of the one --project names
    #[arg(long)]
    all: bool,
    /// Return at most N sessions; has_more tells whether more matched
    #[arg(
        long,
        value_name = "N",
        default_value_t = DEFAULT_LIMIT,
        allow_negative_numbers = true,
        value_parser = count
    )]
    limit: usize,
    /// One session per line: id, a tab, the title (\, tab, LF and CR escaped as \\, \t, \n, \r). Conflicts with --json
    #[arg(long)]
    plain: bool,
    /// Also return sessions started by `claude -p` or the Agent SDK (interactive: false); without it they are left out unless pinned
    #[arg(long)]
    include_headless: bool,
    /// Only sessions with at least N prompts a person typed; pinned ones included
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        allow_negative_numbers = true,
        value_parser = count
    )]
    min_prompts: usize,
}

/// Negative numbers are let through clap so they get this message instead of
/// its hint to pass them after `--`, which would make them a positional.
fn count(raw: &str) -> std::result::Result<usize, String> {
    raw.parse()
        .map_err(|_| "expected a whole number, 0 or more".to_string())
}

impl Listing {
    /// Headless sessions are mostly one-shot automation; a pin is the user's
    /// word that one matters.
    fn shows(&self, row: &Row) -> bool {
        (row.interactive != Some(false) || row.is_pinned || self.include_headless)
            && row.prompts >= self.min_prompts
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// List sessions: pinned first, then newest first
    #[command(
        long_about = "List sessions: pinned first, then newest first by transcript \
        modification time, ties by id. Sessions started by `claude -p` or the Agent SDK are left \
        out unless pinned or --include-headless is given. The window is the first --limit \
        sessions of that order."
    )]
    List {
        #[command(flatten)]
        listing: Listing,
    },
    /// Search titles, prompts and replies, best match first
    #[command(
        long_about = "Search titles, prompts and replies, best match first. All words \
        of the query must come up together in one title, one prompt or one reply, as \
        substrings; a title hit weighs x3, a prompt x2, a reply x1. Title words also match with typos (one edit for 5-6 letters, two from 7); \
        such a hit (fuzzy: true) ranks below every exact one, closer typos first, and is left \
        out when some title matches the words as written. Ties go to the most recently updated session, \
        then the id. matched_in names the best-scoring field; --in limits the fields searched. \
        Sessions started by `claude -p` or the Agent SDK are left out unless pinned or \
        --include-headless is given. The window is the first --limit hits of that order."
    )]
    Search {
        /// Words to find; all of them must come up in the same title, prompt or reply
        query: String,
        /// Where to look: title, prompt, assistant; repeat the flag or separate with commas
        #[arg(
            long = "in",
            value_name = "FIELD",
            value_enum,
            value_delimiter = ',',
            action = clap::ArgAction::Append,
            default_values_t = ALL_FIELDS
        )]
        fields: Vec<MatchField>,
        #[command(flatten)]
        listing: Listing,
    },
    /// Show one session: its row plus first and last prompts, branch, model
    Get {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
        /// Look the session up in the archive
        #[arg(long)]
        archived: bool,
    },
    /// Write a session's prompts and replies as Markdown
    #[command(
        long_about = "Write a session's prompts and replies as Markdown: a header, then \
        `## You · turn N` and `## Claude` sections in file order, tool calls counted, tool output \
        left out. Turn N is the Nth prompt with the replies after it, numbered as excerpts \
        numbers them. Markdown is the default output; --json wraps it in a JSON document."
    )]
    Export {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
        /// Look the session up in the archive
        #[arg(long)]
        archived: bool,
        /// Only these turns: N, A-B, or A- for A to the end. Conflicts with --last-turns
        #[arg(long, value_name = "RANGE", value_parser = parse_range)]
        turns: Option<Selection>,
        /// Only the last N turns. Conflicts with --turns
        #[arg(
            long,
            value_name = "N",
            conflicts_with = "turns",
            allow_negative_numbers = true,
            value_parser = count
        )]
        last_turns: Option<usize>,
        /// Write the result to this file instead of stdout: the Markdown, or the JSON document with --json. A leading ~ is the home directory; missing parent directories are created; an existing file is replaced
        #[arg(long, value_name = "PATH")]
        output_file: Option<String>,
    },
    /// Find where words come up inside one session: turn, prompt or reply, snippet
    #[command(
        long_about = "Find where words come up inside one session, in conversation order: \
        each prompt and each reply that holds every word of the query (case-insensitive \
        substrings) gives its turn number, its role (prompt or assistant) and a snippet. \
        Read around a hit with sessile export ID --turns A-B."
    )]
    Excerpts {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
        /// Words to find; all of them must come up in the same prompt or reply
        query: String,
        /// Look the session up in the archive
        #[arg(long)]
        archived: bool,
        /// Return at most N excerpts; has_more tells whether more matched
        #[arg(
            long,
            value_name = "N",
            default_value_t = DEFAULT_LIMIT,
            allow_negative_numbers = true,
            value_parser = count
        )]
        limit: usize,
        /// One excerpt per line: turn, a tab, the role, a tab, the snippet (escaped as list --plain escapes titles). Conflicts with --json
        #[arg(long)]
        plain: bool,
    },
    /// Give a session a custom title, as /rename does
    #[command(
        long_about = "Give a session a custom title: append the custom-title entry /rename \
        writes. A session that already has that custom title is left as it is (changed: false)."
    )]
    Rename {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
        /// The new title, one line of at most 200 characters; several words are joined with spaces
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
    },
    /// Keep a session at the top of list and out of delete-empty
    Pin {
        /// Session id (a UUID, as list prints it); the session may be live or archived
        #[arg(value_parser = lower)]
        id: String,
    },
    /// Drop a session's pin
    Unpin {
        /// Session id (a UUID); the session need not exist any more
        #[arg(value_parser = lower)]
        id: String,
    },
    /// Move a session's files into the archive, out of /resume
    #[command(
        long_about = "Move a session's files into session-archive/<project>/, out of \
        /resume and of Claude Code's cleanup. restore moves them back. An already archived \
        session is left as it is (changed: false)."
    )]
    Archive {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
    },
    /// Move an archived session's files back
    #[command(
        long_about = "Move an archived session's files back where they were. Refuses with \
        conflict, moving nothing, when any of those paths exists. A session that is not \
        archived any more is left as it is (changed: false)."
    )]
    Restore {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
    },
    /// Delete one session's files for good; needs --yes
    #[command(
        long_about = "Delete one session's files for good: every file keyed by its id, \
        and its pin. Needs --yes; --dry-run lists the paths and changes nothing. A repeat \
        fails with not_found and changes nothing. The prompts typed in the session stay in \
        Claude Code's history.jsonl (the up-arrow history), which sessile never edits."
    )]
    Delete {
        /// Session id (a UUID, as list prints it)
        #[arg(value_parser = lower)]
        id: String,
        /// Look the session up in the archive
        #[arg(long)]
        archived: bool,
        /// List the paths that would be removed; change nothing
        #[arg(long)]
        dry_run: bool,
        /// Confirm the deletion; ignored with --dry-run
        #[arg(long)]
        yes: bool,
    },
    /// Delete every session of the project with no prompt and no reply; needs --yes
    #[command(
        long_about = "Delete every empty session of the --project project: no prompt, no reply \
        and no attachment. A session started by a slash command or a screenshot has no counted \
        prompt but is kept once it has a reply. \
        Pinned, current, live and unreadable sessions are skipped and reported. Needs --yes \
        when there is anything to delete; --dry-run lists it and changes nothing."
    )]
    DeleteEmpty {
        /// List what would be deleted; change nothing
        #[arg(long)]
        dry_run: bool,
        /// Confirm the deletion; ignored with --dry-run
        #[arg(long)]
        yes: bool,
    },
    /// Print the command index, or one command's contract, as JSON
    Schema {
        /// Command name, such as delete-empty; none prints the index
        path: Vec<String>,
    },
}

pub fn main_exit() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return usage_exit(&e),
    };
    let structured = structured_errors(cli.json) || matches!(cli.command, Command::Schema { .. });
    interrupt::install(structured);
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report_error(&e, structured);
            ExitCode::from(e.exit_code())
        }
    }
}

/// `--json` before `--` asks for the error object even when parsing failed.
fn wants_json(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json")
}

fn usage_exit(e: &clap::Error) -> ExitCode {
    match e.kind() {
        ErrorKind::DisplayHelp => {
            write_stdout(&e.render().to_string());
            return ExitCode::SUCCESS;
        }
        ErrorKind::DisplayVersion => {
            write_stdout(&format!("{}\n", env!("CARGO_PKG_VERSION")));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let structured =
        structured_errors(wants_json(&args)) || args.first().is_some_and(|a| a == "schema");
    if !structured {
        let _ = e.print();
        return ExitCode::from(EXIT_USAGE);
    }
    let rendered = e.render().to_string();
    let mut lines = rendered.lines().map(str::trim).filter(|l| !l.is_empty());
    let message = lines.next().map_or("invalid arguments", |l| {
        l.strip_prefix("error: ").unwrap_or(l)
    });
    let mut hint = lines.find_map(|l| l.strip_prefix("tip: ")).map_or_else(
        || "Run `sessile --help` or `sessile schema`".to_string(),
        str::to_string,
    );
    // clap cannot tell a mistyped flag from a query or a title that starts with a dash.
    if e.kind() == ErrorKind::UnknownArgument {
        hint.push_str("; text that starts with '-' (a query, a title) goes last, after `--`");
    }
    report_error(&SessileError::usage(message).with_hint(hint), true);
    ExitCode::from(EXIT_USAGE)
}

/// The F3 object as the last stderr line, or `Error:`/`Hint:` lines for a person.
pub fn report_error(e: &SessileError, structured: bool) {
    if structured {
        eprintln!("{}", e.to_json());
        return;
    }
    eprintln!("Error: {}", escape_controls(&e.message));
    if let Some(paths) = e.context.get("paths").and_then(|p| p.as_array()) {
        for p in paths.iter().filter_map(|p| p.as_str()) {
            eprintln!("  {}", escape_controls(p));
        }
    }
    if let Some(hint) = &e.hint {
        eprintln!("Hint: {}", escape_controls(hint));
    }
}

/// Live sessions plus the current one, so a UI locks it even before Claude
/// Code has written its `sessions/<pid>.json`.
fn mark_current<'a>(rows: impl IntoIterator<Item = &'a mut Row>, current: Option<&str>) {
    for row in rows {
        row.is_current = current == Some(row.id.as_str());
    }
}

fn live_set(config: &Path, current: Option<&str>) -> HashSet<String> {
    let mut live = live_ids(config);
    live.extend(current.map(String::from));
    live
}

fn project_arg(project: Option<&String>) -> Result<String> {
    match project {
        Some(p) => Ok(p.clone()),
        None => std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .map_err(|e| {
                SessileError::usage(format!("no --project and no current directory: {e}"))
                    .with_hint("Pass --project DIR")
            }),
    }
}

/// The format of a command with `--plain`; both flags at once is a usage error.
fn listing_format(json: bool, plain: bool) -> Result<Format> {
    if json && plain {
        return Err(
            SessileError::usage("--plain and --json both select the output format")
                .with_hint("Pass only one of them"),
        );
    }
    Ok(resolve(json, plain, false))
}

fn run(cli: Cli) -> Result<()> {
    if let Command::Schema { path } = &cli.command {
        return introspect::print(path);
    }
    let config = config_dir()?;
    // The slug of an explicit --project scopes id lookups; without it they search every project.
    let scope = match cli.project.as_deref() {
        Some(dir) => Some(project_slug(&config, dir)?),
        None => None,
    };
    let current = cli.current.clone().or_else(|| {
        std::env::var(CURRENT_SESSION_ENV)
            .ok()
            .filter(|id| !id.is_empty())
            .map(|id| id.to_ascii_lowercase())
    });
    let ctx = ops::Ctx {
        config: &config,
        current: current.as_deref(),
        project: scope.as_deref(),
    };
    let json = cli.json;
    let slugs = |listing: &Listing| -> Result<Vec<String>> {
        if listing.all {
            return Ok(project_slugs(&config, listing.archived));
        }
        Ok(vec![project_slug(
            &config,
            &project_arg(cli.project.as_ref())?,
        )?])
    };
    let fmt = resolve(json, false, false);
    match cli.command {
        Command::List { listing } => {
            let fmt = listing_format(json, listing.plain)?;
            cmd_list(&ctx, &slugs(&listing)?, &listing, fmt)
        }
        Command::Search {
            query,
            fields,
            listing,
        } => {
            let fmt = listing_format(json, listing.plain)?;
            cmd_search(&ctx, &slugs(&listing)?, (&query, &fields), &listing, fmt)
        }
        Command::Get { id, archived } => cmd_get(&ctx, &id, archived, fmt),
        command @ (Command::Export { .. } | Command::Excerpts { .. }) => {
            run_turns(&ctx, command, json)
        }
        Command::Rename { id, title } => cmd_rename(&ctx, &id, &title.join(" "), fmt),
        Command::Pin { id } => {
            let changed = pins::pin(&config, &id, ctx.project)?;
            emit_pin(fmt, &id, true, changed);
            Ok(())
        }
        Command::Unpin { id } => {
            let changed = pins::unpin(&config, &id)?;
            emit_pin(fmt, &id, false, changed);
            Ok(())
        }
        Command::Archive { id } => {
            let moved = ops::archive(&ctx, &id)?;
            emit_moved(fmt, "archived", &id, &moved);
            Ok(())
        }
        Command::Restore { id } => {
            let moved = ops::restore(&ctx, &id)?;
            emit_moved(fmt, "restored", &id, &moved);
            Ok(())
        }
        Command::Delete {
            id,
            archived,
            dry_run,
            yes,
        } => {
            let deletion = ops::delete(&ctx, &id, archived, Confirm { dry_run, yes })?;
            emit_deleted(fmt, &deletion);
            Ok(())
        }
        Command::DeleteEmpty { dry_run, yes } => {
            let slug = project_slug(&config, &project_arg(cli.project.as_ref())?)?;
            let report = ops::delete_empty(&config, &slug, ctx.current, Confirm { dry_run, yes })?;
            emit_empty(fmt, &report);
            Ok(())
        }
        Command::Schema { .. } => unreachable!("handled before the config is read"),
    }
}

/// A bounded collection: `items`, `has_more`, and whether a transcript could
/// not be read (`partial`, with the files in `unreadable`).
#[derive(Serialize)]
struct Page<'a, T> {
    items: &'a [T],
    has_more: bool,
    partial: bool,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    unreadable: &'a [Unreadable],
    #[serde(skip)]
    pins_error: Option<SessileError>,
}

impl<'a, T> Page<'a, T> {
    /// `pins_error` is why the pins could not be read: the rows are all there
    /// but none is marked pinned, so the page is partial too.
    fn new(
        items: &'a [T],
        has_more: bool,
        unreadable: &'a [Unreadable],
        pins_error: Option<SessileError>,
    ) -> Self {
        Self {
            items,
            has_more,
            partial: !unreadable.is_empty() || pins_error.is_some(),
            unreadable,
            pins_error,
        }
    }
}

/// The pins for a command that only reads. A pins file that cannot be read
/// marks nothing and fails the call after its result is written, so one
/// broken file does not hide every session.
fn pins_for_read(config: &Path) -> (BTreeSet<String>, Option<SessileError>) {
    match pins::load(config) {
        Ok(pinned) => (pinned, None),
        Err(e) => (BTreeSet::new(), Some(e)),
    }
}

fn lower(id: &str) -> std::result::Result<String, std::convert::Infallible> {
    Ok(id.to_ascii_lowercase())
}

/// Writes a page in the chosen format. A page missing unreadable sessions is
/// still printed, then fails the call with the files named.
fn emit_page<T: Serialize>(
    fmt: Format,
    page: Page<T>,
    line: impl Fn(&T) -> String,
    plain: impl Fn(&T) -> String,
) -> Result<()> {
    match fmt {
        Format::Json => write_json(&page),
        Format::Plain => {
            let text: String = page.items.iter().map(|i| plain(i) + "\n").collect();
            write_stdout(&text);
        }
        _ => {
            let mut text: String = page.items.iter().map(|i| line(i) + "\n").collect();
            if page.has_more {
                text.push_str(&format!(
                    "(first {}; more match: raise --limit)\n",
                    page.items.len()
                ));
            }
            write_stdout(&text);
        }
    }
    if fmt == Format::Plain && page.has_more {
        eprintln!(
            "sessile: first {} only; more match: raise --limit",
            page.items.len()
        );
    }
    if page.unreadable.is_empty() {
        return page.pins_error.map_or(Ok(()), Err);
    }
    let paths: Vec<&str> = page.unreadable.iter().map(|u| u.path.as_str()).collect();
    Err(SessileError::new(
        Kind::IoError,
        format!(
            "{} transcript(s) could not be read and are left out: {}",
            paths.len(),
            page.unreadable[0].message
        ),
    )
    .with_hint("Check the files' permissions; the result lists the sessions that were read")
    .with_context("paths", json!(paths)))
}

fn plain_row(row: &Row) -> String {
    format!("{}\t{}", row.id, plain_field(&row.title))
}

fn cmd_list(ctx: &ops::Ctx, slugs: &[String], listing: &Listing, fmt: Format) -> Result<()> {
    let live = live_set(ctx.config, ctx.current);
    let (mut rows, unreadable) = list_projects(ctx.config, slugs, listing.archived, &live);
    let (pinned, pins_error) = pins_for_read(ctx.config);
    pins::mark(&mut rows, &pinned);
    mark_current(&mut rows, ctx.current);
    rows.retain(|r| listing.shows(r));
    // Before the limit, so pins survive it.
    pins::pinned_first(&mut rows);
    let has_more = rows.len() > listing.limit;
    rows.truncate(listing.limit);
    emit_page(
        fmt,
        Page::new(&rows, has_more, &unreadable, pins_error),
        row_line,
        plain_row,
    )
}

fn cmd_search(
    ctx: &ops::Ctx,
    slugs: &[String],
    (query, fields): (&str, &[MatchField]),
    listing: &Listing,
    fmt: Format,
) -> Result<()> {
    let live = live_set(ctx.config, ctx.current);
    let (mut hits, unreadable) = search(ctx.config, slugs, listing.archived, query, fields, &live)?;
    // Hits stay in relevance order; the pin is only marked.
    let (pinned, pins_error) = pins_for_read(ctx.config);
    pins::mark(hits.iter_mut().map(|h| &mut h.row), &pinned);
    mark_current(hits.iter_mut().map(|h| &mut h.row), ctx.current);
    hits.retain(|h| listing.shows(&h.row));
    // A word some title has as written is not a typo: "sessile" must not
    // bring "session" along. Exact prompt or reply hits do not count, so a
    // misspelling quoted in a conversation still finds the title.
    if hits.iter().any(|h| h.matched_in == "title" && !h.fuzzy) {
        hits.retain(|h| !h.fuzzy);
    }
    let has_more = hits.len() > listing.limit;
    hits.truncate(listing.limit);
    let page = Page::new(&hits, has_more, &unreadable, pins_error);
    let line = |h: &crate::search::Hit| {
        format!(
            "{}  [{} {}]  {}",
            row_line(&h.row),
            h.matched_in,
            h.score,
            escape_controls(&h.snippet)
        )
    };
    emit_page(fmt, page, line, |h| plain_row(&h.row))
}

fn cmd_get(ctx: &ops::Ctx, id: &str, archived: bool, fmt: Format) -> Result<()> {
    let located = locate(ctx.config, id, archived, ctx.project)?;
    let mut detail = show(&located, id, &live_set(ctx.config, ctx.current))?;
    let (pinned, pins_error) = pins_for_read(ctx.config);
    let after = pins_error.map_or(Ok(()), Err);
    pins::mark([&mut detail.row], &pinned);
    mark_current([&mut detail.row], ctx.current);
    if fmt == Format::Json {
        write_json(&detail);
        return after;
    }
    let field = |v: Option<&str>| escape_controls(v.unwrap_or("-")).into_owned();
    let mut out = vec![
        row_line(&detail.row),
        format!("path:    {}", field(Some(&detail.path))),
        format!("cwd:     {}", field(detail.row.cwd.as_deref())),
        format!("branch:  {}", field(detail.git_branch.as_deref())),
        format!("model:   {}", field(detail.model.as_deref())),
        format!("updated: {}", detail.row.updated_at),
    ];
    out.extend(
        detail
            .first_prompts
            .iter()
            .map(|p| format!("first:   {}", escape_controls(p))),
    );
    out.extend(
        detail
            .last_prompts
            .iter()
            .map(|p| format!("last:    {}", escape_controls(p))),
    );
    write_stdout(&(out.join("\n") + "\n"));
    after
}

/// Markdown is the export's native document: written as it is to a file or a
/// pipe, with control sequences escaped only on a terminal.
fn cmd_export(
    ctx: &ops::Ctx,
    (id, archived): (&str, bool),
    selection: Option<Selection>,
    output_file: Option<&str>,
    fmt: Format,
) -> Result<()> {
    let located = locate(ctx.config, id, archived, ctx.project)?;
    let text = markdown(&located, id, &live_set(ctx.config, ctx.current), selection)?;
    let result = if fmt == Format::Json {
        let mut doc =
            serde_json::to_string(&document(ctx.config, id, text)?).expect("a document encodes");
        doc.push('\n');
        doc
    } else {
        text
    };
    match output_file {
        Some(path) => write_result_file(&expand_home(path), &result),
        None if fmt != Format::Json && std::io::IsTerminal::is_terminal(&std::io::stdout()) => {
            write_stdout(&escape_controls(&result));
            Ok(())
        }
        None => {
            write_stdout(&result);
            Ok(())
        }
    }
}

/// The commands that read one session turn by turn.
fn run_turns(ctx: &ops::Ctx, command: Command, json: bool) -> Result<()> {
    match command {
        Command::Export {
            id,
            archived,
            turns,
            last_turns,
            output_file,
        } => cmd_export(
            ctx,
            (&id, archived),
            turns.or(last_turns.map(Selection::Last)),
            output_file.as_deref(),
            resolve(json, false, true),
        ),
        Command::Excerpts {
            id,
            query,
            archived,
            limit,
            plain,
        } => cmd_excerpts(
            ctx,
            (&id, archived),
            (&query, limit),
            listing_format(json, plain)?,
        ),
        _ => unreachable!("run passes only export and excerpts"),
    }
}

fn cmd_excerpts(
    ctx: &ops::Ctx,
    (id, archived): (&str, bool),
    (raw, limit): (&str, usize),
    fmt: Format,
) -> Result<()> {
    let query = Query::new(raw).ok_or_else(|| {
        SessileError::usage("empty search query").with_hint("Pass at least one word to find")
    })?;
    let located = locate(ctx.config, id, archived, ctx.project)?;
    let path = located.transcript(id);
    let map = Mapped::open(&path).map_err(|e| SessileError::io("read", &path, &e))?;
    let mut found = excerpts(&exchanges(turns(map.bytes())), &query);
    let has_more = found.len() > limit;
    found.truncate(limit);
    let line = |e: &Excerpt| {
        format!(
            "turn {:>4}  {:<9}  {}",
            e.turn,
            e.role,
            escape_controls(&e.snippet)
        )
    };
    let plain = |e: &Excerpt| format!("{}\t{}\t{}", e.turn, e.role, plain_field(&e.snippet));
    emit_page(fmt, Page::new(&found, has_more, &[], None), line, plain)
}

/// The result stdout would have carried, written beside and renamed over.
fn write_result_file(path: &Path, text: &str) -> Result<()> {
    let io_err = |op: &str, p: &Path, e: std::io::Error| SessileError::io(op, p, &e);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".sessile-tmp");
    let tmp = Path::new(&tmp);
    fs::write(tmp, text).map_err(|e| io_err("write", tmp, e))?;
    fs::rename(tmp, path).map_err(|e| io_err("rename", path, e))
}

fn cmd_rename(ctx: &ops::Ctx, id: &str, title: &str, fmt: Format) -> Result<()> {
    let changed = ops::rename(ctx, id, title)?;
    let title = title.trim();
    if fmt == Format::Json {
        write_json(&json!({"id": id, "title": title, "changed": changed}));
    } else {
        let verb = if changed { "renamed" } else { "already titled" };
        write_stdout(&format!("{verb} {id}: {}\n", escape_controls(title)));
    }
    Ok(())
}

fn emit_pin(fmt: Format, id: &str, pinned: bool, changed: bool) {
    if fmt == Format::Json {
        write_json(&json!({"id": id, "pinned": pinned, "changed": changed}));
        return;
    }
    let state = if pinned { "pinned" } else { "not pinned" };
    let note = if changed { "" } else { " (already)" };
    write_stdout(&format!("{id} {state}{note}\n"));
}

fn emit_moved(fmt: Format, verb: &str, id: &str, moved: &[ops::Moved]) {
    let changed = !moved.is_empty();
    if fmt == Format::Json {
        write_json(&json!({"id": id, "moved": moved, "changed": changed}));
        return;
    }
    if !changed {
        write_stdout(&format!("{id} is already {verb}\n"));
        return;
    }
    let mut out = vec![format!("{verb} {id} ({} paths)", moved.len())];
    out.extend(moved.iter().map(|m| {
        format!(
            "  {} -> {}",
            escape_controls(&m.from),
            escape_controls(&m.to)
        )
    }));
    write_stdout(&(out.join("\n") + "\n"));
}

fn emit_deleted(fmt: Format, deletion: &ops::Deletion) {
    if fmt == Format::Json {
        write_json(deletion);
        return;
    }
    let verb = if deletion.changed {
        "removed"
    } else {
        "would remove"
    };
    let mut out = Vec::new();
    for t in &deletion.targets {
        out.push(format!("{verb} {} paths of {}", t.paths.len(), t.id));
        out.extend(t.paths.iter().map(|p| format!("  {}", escape_controls(p))));
    }
    if deletion.requires_confirmation == Some(true) {
        out.push("(needs --yes)".into());
    }
    write_stdout(&(out.join("\n") + "\n"));
}

fn emit_empty(fmt: Format, report: &ops::EmptyReport) {
    if fmt == Format::Json {
        write_json(report);
        return;
    }
    let verb = if report.changed {
        "deleted"
    } else {
        "would delete"
    };
    let mut out = vec![format!("{verb} {} empty sessions", report.targets.len())];
    for t in &report.targets {
        out.push(format!("  {}  {}", t.row.id, escape_controls(&t.row.title)));
        out.extend(
            t.paths
                .iter()
                .map(|p| format!("    {}", escape_controls(p))),
        );
    }
    out.extend(
        report
            .skipped
            .iter()
            .map(|s| format!("  skipped {}: {}", s.id, escape_controls(&s.reason))),
    );
    if report.requires_confirmation == Some(true) {
        out.push("(needs --yes)".into());
    }
    write_stdout(&(out.join("\n") + "\n"));
}

fn row_line(row: &Row) -> String {
    let marker = match (row.is_pinned, row.is_live) {
        (true, true) => "^*",
        (true, false) => "^ ",
        (false, true) => " *",
        (false, false) => "  ",
    };
    let title = if row.title.is_empty() {
        "(untitled)".into()
    } else {
        escape_controls(&row.title)
    };
    format!(
        "{} {marker} {:>4}p {:>8} {:>5}  {title}",
        row.id.get(..8).unwrap_or(&row.id),
        row.prompts,
        human_size(row.size_bytes),
        age(row.updated_ms),
    )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn age(updated_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    let secs = now.saturating_sub(updated_ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// The parser, for introspection.
pub fn command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}
