# sessile CLI

Engine of the sessile mod: lists, searches, renames, archives, restores, exports and deletes local Claude Code sessions. Linux, macOS and Windows.

Build: `cargo build --release`, binary at `target/release/sessile`. Install: `cargo install --path . --locked`.

`CLAUDE_CONFIG_DIR` overrides `.claude` in the home directory (`USERPROFILE` on Windows).

`sessile schema` describes every command for a program (see [Introspection](#introspection)).

## Global flags

Accepted by every subcommand, before or after it:

- `--json`: the result as one JSON document on stdout. It is also the default when stdout is not a terminal; at a terminal the default is a human-readable table.
- `--project <dir>`: the cwd the sessions were started in (Claude Code's `projects/<slug>` dir, slug = path with every non-alphanumeric character replaced by `-`). Defaults to the current directory. `list`, `search` and `delete-empty` use it as the project. Commands that take an id (`get`, `rename`, `archive`, `restore`, `delete`, `export`, `excerpts`) look the id up only in that project when `--project` is given (not found otherwise), and in every project when it is absent.
- `--current <id>`: the session you are running in; defaults to `CLAUDE_CODE_SESSION_ID`, which Claude Code sets for the commands it runs, so an agent never has to know its own id. It is refused by every change (`rename`, `archive`, `restore`, `delete`, `delete-empty`) and reported with `is_live: true` and `is_current: true` in `list`, `get` and `search`, so a UI can lock it. Live sessions (a running pid in `sessions/<pid>.json`) are refused the same way. When that file of a running process names no session (it was caught half-written, or it is broken), every change is refused until it reads again: the process holds a session nobody can tell.

`--version` prints the bare version. `--help` on any command names its defaults; `sessile schema` describes the same in JSON.

## Commands

| Command | Does | Effect |
|---|---|---|
| `sessile list [--archived] [--all] [--limit N] [--plain] [--include-headless] [--min-prompts N]` | one row per session, pinned first, then newest first; `--all` covers every project | read-only |
| `sessile search <query> [--in FIELD,…] [--archived] [--all] [--limit N] [--plain] [--include-headless] [--min-prompts N]` | ranked rows with score, matched field and snippet | read-only |
| `sessile get <id> [--archived]` | row plus first/last prompts, branch, model | read-only |
| `sessile excerpts <id> <query> [--archived] [--limit N] [--plain]` | where the words come up inside one session: turn, prompt or reply, snippet | read-only |
| `sessile export <id> [--archived] [--turns RANGE \| --last-turns N] [--output-file PATH]` | prompts and replies as Markdown, all or some turns | read-only |
| `sessile rename <id> <title...>` | appends a `custom-title` entry | idempotent |
| `sessile pin <id>` / `sessile unpin <id>` | keeps a session at the top of `list` and out of `delete-empty` | idempotent |
| `sessile archive <id>` / `sessile restore <id>` | moves the footprint to / from `session-archive/<slug>/` | idempotent |
| `sessile delete <id> [--archived] [--dry-run] [--yes]` | removes the footprint for good | not idempotent, needs `--yes` |
| `sessile delete-empty [--dry-run] [--yes]` | deletes this project's empty sessions: no prompt, no reply, no attachment | idempotent, needs `--yes` |
| `sessile schema [COMMAND]` | the machine-readable description of the CLI | read-only |

## Output

| Format | When | Shape |
|---|---|---|
| text | stdout is a terminal (default) | a table or a few lines; ESC and C1 controls from transcripts are shown as `\u{1b}` |
| JSON | `--json`, or stdout is not a terminal | one document per call, snake_case keys |
| plain | `--plain` (`list`, `search`, `excerpts`) | one session per line: id, a tab, the title (`excerpts`: turn, role, snippet); `\`, tab, LF, CR written as `\\`, `\t`, `\n`, `\r`; `--plain` with `--json` is a usage error |
| Markdown | `export` without `--json`, on any stdout | the transcript; escaped only at a terminal |

A reader that closes the pipe early ends the call with exit 0 and no message.

## Errors and exit codes

| Exit | Meaning |
|---|---|
| 0 | success |
| 1 | failure (`not_found`, `conflict`, `confirmation_required`, `session_live`, `pins_unreadable`, `io_error`) |
| 2 | usage error (`invalid_input`) |
| 130 | interrupted by Ctrl-C (`interrupted`) |

A failure prints nothing on stdout, except a `partial` page of `list` or `search`. On stderr it is one JSON object on the last line when `--json` is given or either stdout or stderr is not a terminal, and `Error:` / `Hint:` lines otherwise (a person at a terminal, nothing piped):

```json
{"error":{"kind":"session_live","message":"cannot archive the current session 0a1b2c3d-1111-4222-8333-444455556666","hint":"Run the command from another session","context":{"id":"0a1b2c3d-1111-4222-8333-444455556666"}}}
```

| `kind` | When | `context` |
|---|---|---|
| `invalid_input` | bad flag or argument; the hint carries clap's tip | |
| `not_found` | no session with that id here (the hint suggests `--archived` or dropping it) | `id` |
| `conflict` | `restore`/`archive` targets already exist; nothing is moved | `paths` |
| `confirmation_required` | `delete`/`delete-empty` without `--yes` | |
| `session_live` | the current session or one a Claude Code process runs | `id` |
| `pins_unreadable` | `<config>/sessile/pins.json` does not parse; it is never overwritten. `list`, `search` and `get` still print their result (nothing marked pinned, `partial: true`) before failing | `path` |
| `io_error` | a file operation failed, or `list`/`search` could not read a transcript (the page is still printed, with `partial: true`) | `path` or `paths` |
| `outcome_unknown` | `rename` failed while appending the title entry: the title may or may not have changed, `get` tells | `id`, `path` |
| `interrupted` | Ctrl-C | `completed` |

Ctrl-C during a read stops at once. During a change it stops between two files: `archive`/`restore` move back what they had moved, and anything they could not move back, or what `delete` had already removed, is listed in `context.completed`.

## Introspection

`sessile schema` (no config needed) prints the index:

```json
{"schema_version":"1","tool_version":"0.1.0",
 "commands":["archive","delete","delete-empty","excerpts","export","get","list","pin","rename","restore","search","unpin"],
 "global_flags":[…],"format_defaults":{"tty":"text","non_tty":"json"},
 "exit_codes":{"0":"success","1":"failure","2":"usage error","130":"interrupted by Ctrl-C"}}
```

`sessile schema <command>` prints that command's arguments and flags (type, default, required), `effects` (`read_only`, `idempotent`, `non_idempotent`), `confirm`, `interactive: false`, format defaults where they differ, and the JSON Schema of its output (`type`, `enum`, `properties`, `required`, `items`). Flags and defaults are read from the parser itself, so the description cannot drift from what the binary accepts. The schemas are the reference for the JSON below.

## JSON

Session ids are UUIDs. Times are RFC 3339 UTC with milliseconds. Every command that changes something reports `changed`.

### `list`, `search`

```json
{"items":[{"id":"0a1b2c3d-1111-4222-8333-444455556666","title":"Fix flaky login test","title_source":"ai",
  "updated_at":"2026-10-03T14:10:43.200Z","created_at":"2026-10-03T14:02:11.518Z","size_bytes":450,"prompts":1,"is_live":false,"is_current":false,"is_pinned":false,
  "first_prompt":"Fix the flaky login test","cwd":"/work/proj",
  "resume_command":"cd '/work/proj' && claude --resume 0a1b2c3d-1111-4222-8333-444455556666"}],
 "has_more":false,"partial":false}
```

- `--limit` defaults to 50; `has_more` tells whether more sessions matched. Text and plain output print a note when the list was cut (plain on stderr, so stdout stays one session per line).
- `title_source`: `custom` (last `custom-title` entry), `ai` (last `ai-title`), `prompt` (first prompt cut to 80 characters), `none` (`title` is `""`).
- `updated_at` is the transcript's modification time (last activity); `created_at` is when the session started, the `timestamp` of its first entry with a `cwd`, `null` when there is none.
- `size_bytes`: transcript plus its `<id>/` sidecar dir.
- `prompts`: user messages with text that are not injected by Claude Code (tool results, meta entries, compact summaries, `<command-name>`, `<local-command-`, `<system-reminder>`, `Caveat:` and similar are not counted).
- `first_prompt`: whitespace collapsed, cut to 300 characters, `null` when there is none.
- `cwd`: the first directory the transcript records, `null` when there is none.
- `resume_command`: what to paste into a shell to resume where the session ran; PowerShell quoting on Windows (`cd '…'; claude --resume …`), POSIX elsewhere; `claude --resume <id>` without a `cwd`.
- `--min-prompts N` keeps only sessions with at least N prompts, pinned ones included: one-shot automation rarely has more than one or two.
- `interactive`: `false` when the transcript's first entry with a `cwd` records an `sdk-*` entrypoint (`claude -p`, the Agent SDK), `true` for any other entrypoint, `null` when there is none: Claude Code before 2.1.78 did not write it, and nothing else in those transcripts tells the two apart (checked over ~3 500 local transcripts). `list` and `search` leave out only `false`, unless the session is pinned or `--include-headless` is given.
- `partial`: `true` when a transcript exists but cannot be read. Those sessions are not in `items` (nothing is guessed about them), `unreadable` names them as `[{"path","message"}]`, and the call still prints the page but exits 1 with an `io_error` whose `context.paths` lists the files. `unreadable` is present only then.
- `search` rows add `score`, `matched_in` (`title`, `prompt`, `assistant`) and `snippet` (up to 100 characters around the match). Ranking: all query words must come up together in one title, one prompt or one reply, as substrings; a title hit weighs x3, a prompt x2, a reply x1. The title is the one the row shows (custom, else AI; a title made from the first prompt matches as that prompt). A title also matches with typos: each query word of 5–6 letters may be one edit (insertion, deletion, substitution or swap of neighbours) from a word of the title, 7 or more letters two edits, shorter words must match as written. Such a hit is marked `fuzzy: true`, scores below the lowest possible exact score so every exact hit ranks first, and loses score per edit so closer typos rank higher. Typo hits are left out when some listed title matches the query as written (otherwise "sessile" would also bring "session"); exact hits in prompts or replies do not count, so a misspelling quoted in a conversation still finds the title. `matched_in` is the best-scoring field. `--in title,prompt` (or a repeated `--in`) limits the fields searched, e.g. to skip sessions that only mention the words in replies. Ties go to the most recently updated session, then the id. `search` marks `is_pinned` but keeps relevance order.

### `get`

The `list` row plus:

```json
{"first_prompts":["Fix the flaky login test"],"last_prompts":["Fix the flaky login test"],
 "git_branch":"main","model":"claude-opus-5-5","version":"2.1.288",
 "path":"/home/me/.claude/projects/-work-proj/0a1b2c3d-1111-4222-8333-444455556666.jsonl"}
```

Up to 3 first and 3 last prompts (they overlap in short sessions). `git_branch`, `model` (never `<synthetic>`) and `version` are the latest. Any of them can be `null`.

### `rename`

```json
{"id":"0a1b2c3d-1111-4222-8333-444455556666","title":"Login test fix","changed":true}
```

A session that already has that custom title is left alone (`changed: false`).

### `pin`, `unpin`

```json
{"id":"0a1b2c3d-1111-4222-8333-444455556666","pinned":true,"changed":true}
```

Pins live in `<config>/sessile/pins.json` (`{"pinned":[ids]}`), keyed by id, so they follow a session into the archive and back; Claude Code never reads them, so `/resume` does not show them. `pin` needs the session to exist (live or archived); `unpin` takes any id. `delete` drops the pin; `delete-empty` skips pinned sessions. `pin` and `unpin` hold a lock (`sessile/pins.lock`) while they rewrite the file, so commands run at the same time keep each other's pins.

### `archive`, `restore`

```json
{"id":"0a1b2c3d-1111-4222-8333-444455556666","changed":true,
 "moved":[{"from":"/home/me/.claude/debug/0a1b2c3d-1111-4222-8333-444455556666.txt",
           "to":"/home/me/.claude/session-archive/-work-proj/debug/0a1b2c3d-1111-4222-8333-444455556666.txt"}]}
```

Archive keeps each path relative to the config dir under `session-archive/<slug>/`, outside `projects/`, so Claude Code's `cleanupPeriodDays` sweep does not reach it. A session already where the command puts it gives `moved: []`, `changed: false`. A target that exists fails with `conflict` and moves nothing.

### `delete`

```json
{"targets":[{"id":"0a1b2c3d-1111-4222-8333-444455556666",
  "paths":["/home/me/.claude/debug/0a1b2c3d-1111-4222-8333-444455556666.txt",
           "/home/me/.claude/projects/-work-proj/0a1b2c3d-1111-4222-8333-444455556666.jsonl"]}],
 "changed":true}
```

`--dry-run` lists the same paths with `changed: false` and `requires_confirmation: true`. Without `--dry-run` it needs `--yes`, checked after every other check (an unknown or live id fails with its own kind first). A repeat fails with `not_found`.

### `delete-empty`

```json
{"targets":[{"id":"9f8e7d6c-1111-4222-8333-444455556666","title":"","title_source":"none", "…":"…",
             "paths":["/home/me/.claude/projects/-work-proj/9f8e7d6c-1111-4222-8333-444455556666.jsonl"]}],
 "skipped":[{"id":"…","reason":"pinned"}],
 "changed":true}
```

A session is empty when it has 0 prompts, no assistant message and no attachment in a user message. A session started by a slash command, a screenshot or `!` input counts 0 prompts but is not empty once anything followed, and is left alone without being reported. A transcript line that does not parse counts as content.

`targets` are `list` rows plus `paths`. Current, live, pinned and unreadable sessions with 0 prompts land in `skipped` instead of failing the run; `reason` is `pinned`, `unreadable: …` or the refusal's message. `--dry-run` adds `requires_confirmation` (true when `targets` is not empty). `--yes` is needed only when there is something to delete.

### Turns

Turn N is the Nth prompt a person typed (the prompts `prompts` counts) with the replies after it; replies before the first prompt, when there are any, are turn 0. `excerpts` and `export` number turns the same way, so a hit from one is read with the other.

### `excerpts`

```json
{"items":[{"turn":9,"role":"assistant","snippet":"…adapters exist (codex-acp, claude-agent-acp, gemini --acp)…"}],
 "has_more":true,"partial":false}
```

Every prompt and every reply (consecutive assistant messages joined) that holds all the query's words as case-insensitive substrings, in conversation order. `role` is `prompt` or `assistant`; `snippet` is up to 100 characters around the first word found. `--limit` defaults to 50. `partial` is always `false`.

### `export`

The default output is the Markdown itself, at a terminal or not: a header (title, id, directory, branch, model, prompts, resume command), then `## You · turn N` / `## Claude` sections in file order; consecutive assistant messages form one reply, tool calls are counted (`*Tools: Bash ×2, Read*`), tool output and injected text are left out.

`--turns 12`, `--turns 12-15` or `--turns 12-` (to the end) keep only those turns, `--last-turns N` the last N; the header then adds `- Turns: 12-15 of 328` (`none of 328` when the range is past the end). The two flags conflict.

`--output-file PATH` writes what stdout would have carried to `PATH` instead (a leading `~` is the home dir, missing parents are created, an existing file is replaced) and leaves stdout empty.

With `--json`:

```json
{"id":"0a1b2c3d-1111-4222-8333-444455556666","markdown":"# Fix flaky login test\n…","truncated":false}
```

`markdown` holds at most 256 KiB. A longer export is cut at a character boundary, `truncated` is `true`, and `output_file` names `<config>/sessile/exports/<id>.md` (mode 0600), which holds the whole text until the next truncated export of that session replaces it or `delete` removes it.

## Footprint

Everything keyed by the session id, and nothing else (`projects/<slug>/memory/` is never touched):

`projects/<slug>/<id>.jsonl`, `projects/<slug>/<id>/`, `session-env/<id>/`, `file-history/<id>/`, `tasks/<id>/`, `debug/<id>.txt`, `todos/<id>-*.json`, `security/*<id>*`, `telemetry/*<id>*`, plus sessile's own `sessile/exports/<id>.md`. Only paths that exist are listed.
