# sessile SPEC

Claude Code mod that manages local sessions from inside Claude Code: browse, search, resume, rename, archive, restore, export, delete.
Fills the gap left by `/resume`, which can only rename (`Ctrl+R`), and by `claude purge`, which works per project, not per session.

Status: draft v0.3, 2026-10-04.

## Shape

Two parts in one repo:

- `cli/`: Rust CLI, binary `sessile`. The engine: scans, searches and changes sessions; JSON on stdout. Usable standalone. Contract in `cli/README.md` and in `sessile schema`.
- `mod/`: Claude Code mod (function-hooks plugin), the UI. Slash command `/sessile` (alias `/sessions`) opens a Pane drawn from native elements; every read and change goes through `$.process.run([cliPath, ...])`, always with `--current` and `--json` (except the export, which writes its file with `--output-file`), plus `--project` unless the pane shows all projects. The CLI's snake_case JSON becomes the mod's camelCase types in one adapter (`fromCli` in `hooks/view.ts`).
  It also ships the agent skill `mod/skills/find-session/` (loaded as `sessile:find-session`): how an agent finds and reads past sessions through the CLI, covering only what `--help` does not. CLI flag changes must be checked against it.

Why the split: a mod runs TS with no Node, and `$.fs.read` refuses files over 4 MiB, while transcripts reach hundreds of MB.
Reading head and tail of 500+ files and full-text search need native code.

- Loaded via `CLAUDE_CODE_PLUGIN_DIRS` (in `~/.claude/settings.json` `env`) or `--plugin-dir`.
- Scope: the current project (`~/.claude/projects/<cwd-slug>/`), or every project with `--all` / the pane's `w` toggle.
- Platforms: Linux, macOS, Windows. Linux is the one run so far; macOS and Windows build and pass clippy by cross-compile only.

### sessile CLI

| Command | Does |
|---|---|
| `list [--archived] [--all] [--limit N] [--plain] [--include-headless] [--min-prompts N]` | one row per session (data model below), pinned first, then newest first; `claude -p` / Agent SDK sessions only with the flag or a pin |
| `get <id> [--archived]` | preview: first/last prompts, cwd, branch, model |
| `search <query> [--in FIELD,…] [--archived] [--all] [--limit N] [--plain] [--include-headless] [--min-prompts N]` | ranked: titles, then prompts, then assistant text (substrings); title words also match with typos (1 edit for 5–6 letters, 2 from 7), always below every exact hit and only when no title matches as written; `--in` limits the fields |
| `rename <id> <title>` | appends a `custom-title` entry; the same title again changes nothing |
| `archive <id>` / `restore <id>` | moves the footprint; a session already there is left alone |
| `export <id> [--archived] [--turns RANGE \| --last-turns N] [--output-file PATH]` | Markdown transcript: prompts (`## You · turn N`), replies, tool counts; all turns or the ones asked for; to stdout, or to `PATH` |
| `excerpts <id> <query> [--archived] [--limit N] [--plain]` | turn number, role and snippet of every prompt and reply holding all the words, in conversation order; turn N = Nth counted prompt plus its replies, as in `export` |
| `delete <id> [--archived] [--dry-run] [--yes]` | removes the footprint; needs `--yes`, dry-run lists the paths |
| `delete-empty [--dry-run] [--yes]` | bulk delete of empty sessions (no prompt, no reply, no attachment); guarded, pinned and unreadable ones go to `skipped` |
| `pin <id>` / `unpin <id>` | keeps a session at the top of `list` and out of `delete-empty` |
| `schema [COMMAND]` | machine-readable description of every command |

- `--project`, `--current` and `--json` are global flags. `--project` scopes id lookups to that project; `--current` defaults to `CLAUDE_CODE_SESSION_ID`, which Claude Code sets for the commands it runs.
- Output: text at a terminal, JSON elsewhere or with `--json`, one line per session with `--plain`; `export` prints Markdown unless `--json`.
- `list` and `search` return `{items, has_more, partial}`, 50 by default; times are RFC 3339. A transcript that cannot be read makes the page `partial` and the call exit 1; the pane still shows the rest and says what is missing.
- Every destructive command refuses the `--current` and live sessions itself, so the guard holds outside the mod too.
- Exit codes: 0 ok, 1 failure, 2 usage, 130 Ctrl-C. Errors as one `{"error":{kind,message,hint,context}}` object on stderr (when `--json` is given or stdout or stderr is not a terminal).
- Speed target: `list` for 520 sessions / 1.3 GB under 150 ms warm; `search` under 1 s warm. Measured with `--all` over 3 679 sessions: `list` 0,35 s, `search` 0,6 s.
- The mod caps the all-projects list at 5 000 rows (`--limit`): the engine's stdout limit is 4 MiB, about 500 B per row.

### Pane and keys

A `Client` surface module gets keys only after a mouse click (`$.ui.focus` cannot reach it), so v1 uses native elements: an `Input` filter, rows as plain `Button`s, actions as `Button`s with hotkeys.

- List: type to filter (fuzzy on title and id), `Enter` full-text search, `↑↓`/`Tab` walk rows, `Enter` on a row opens it; a click selects a row and a second click on it opens it (the click that hands the pane its keys only selects); `n`/`b` page down/up (`less`), `x` junk only, `v` show archive / show sessions (the view; `a` on a row archives it), `w` this project / all projects, `e` delete empty (current project, live view only), `c` clear filter or search, `f` back to the filter, `g` reload, `q` close.
- Letter keys act only once the ring has left the filter (a focused Input takes every printable key): their caps are drawn dim until then, and while the pane does not hold the keys at all. While the ring is off the rows, the line under them says what the keys do from there; the marker legend has a line of its own, always drawn in the full layout.
- A low pane (an inline pane gets about a quarter of the screen: 6 rows on a 24-row terminal) uses a compact layout once the full one would show fewer than four sessions: no column header and no legend, `▲`/`▼` share the filter's and the row actions' lines, the keys take one clipped line each (every key still works), and a notice or the "keys are with the prompt" line takes the header's place. The delete dialogs list fewer paths or sessions there, never losing the question or the buttons.
- The footer and the row actions take as many lines as the width needs (three and two in the ~58-column dock); the list's height is planned for the widest labels, so a toggle never moves the rows.
- A toggle that leaves no row keeps the ring on its own button, and an archive that leaves no row puts it on `u`, so the next letter acts instead of typing into the filter.
- Changing the scope (`v`, `w`, `g`) runs an open full-text search again in the new scope.
- All-projects view adds a PROJECT column (last segment of the session's cwd).
- Detail and row actions: `r` resume, `t` rename, `a` archive/restore, `d` delete, `p` pin/unpin, `m` export Markdown, `i` copy id; detail also `Esc` back (a clickable `esc back` button, no letter: `b` pages up in the list), `q` close.
- After archive or restore the notice offers `u` undo (the reverse command), until the next action.
- Confirm dialogs: `y` yes, `n` cancel. `Esc` steps back one level: a filled filter clears first (wherever the ring is), the plain list closes the pane. The keys stay in the pane: Escape hands them to the prompt before the mod hears of it, so the pane asks for them back (granted over an empty composer).
- The ✕ mark and `ctrl+x x` reach the mod as the same close as Escape, so with a filter or a detail open they step back too.
- A key the pane does not bind (`j`, `k`, `?`, `/`, Space) falls through to the prompt and takes the keyboard with it; the mod API has no way to hold it back (a key is bound only by a Button, and every Button is a stop of the focus ring). While the prompt has the keys, the legend's line says so in yellow and how to get them back (a click on the pane, or `/sessile`).
- Key choices follow habits users bring: `j`/`k` stay free (down/up in vim, less, fzf, lazygit), `b` is back a page as in `less`, `p` pins.

### Options (`userConfig`)

| Option | Default | Does |
|---|---|---|
| `cliPath` | `sessile` | binary to run |
| `layout`, `dockBackground` | | pane placement and dock colour |
| `exportDir` | empty | where `m` writes (`~` allowed); empty writes into the directory of the session the pane runs in |

## Data model

A session is `<id>.jsonl` in the project dir. Read per session:

| Field | Source |
|---|---|
| title | last `custom-title` entry, else last `ai-title`, else first user prompt (truncated) |
| updated | jsonl mtime |
| created | `timestamp` of the first entry with a `cwd`; `null` when it has none |
| size | jsonl size + `<id>/` dir |
| prompts | count of non-meta `user` entries with text content |
| cwd | first `cwd` in the head of the transcript |
| resume command | `cd '<cwd>' && claude --resume <id>`; PowerShell quoting on Windows |
| live | `~/.claude/sessions/<pid>.json` has this `sessionId` and the pid is alive. A file of a live pid that names no session blocks every change until it reads again |
| interactive | not an `sdk-*` `entrypoint` on the first entry with a `cwd`; unknown (`null`, never hidden) before Claude Code 2.1.78, which wrote no entrypoint; the pane lists headless sessions too (`--include-headless`), since clearing them out is part of its job |

Listing reads only what it needs: titles are near the end of the file, first prompt and cwd near the start.
Large transcripts (100 MB+) must not be read whole for the list.

### Session footprint

Everything keyed by the session id, used by delete and archive:

- `projects/<proj>/<id>.jsonl` and `projects/<proj>/<id>/` (subagents, tool results)
- `session-env/<id>/`, `file-history/<id>/`, `tasks/<id>/`, `debug/<id>.txt`
- `todos/<id>-*.json`, `security/*<id>*`, `telemetry/*<id>*`

Never touched: `projects/<proj>/memory/`, agent memory, anything not keyed by the id.

## Features (v1)

### List

- Rows: pin, marker (this / live / named), title, updated (relative), prompts, size, short id; PROJECT in the all-projects view.
- Glyphs, one cell each and told apart by shape: `★` pinned (yellow), `◆` this (cyan), `●` live (green), `✎` named (dim). One marker column, first that applies: this, live, named.
- Sort newest first. Text filter on title and id.
- Junk filter (one toggle): a row is junk when it has `≤1 prompt`, `< 20 KB`, or no title at all.

### Preview

- Selected session: first 3 and last 3 user prompts, cwd, git branch, model, full id.

### Rename

- Appends `{"type":"custom-title","customTitle":"<name>","sessionId":"<id>"}` to the jsonl, the same entry `/rename` writes.

### Archive / restore

- Archive moves the footprint to `~/.claude/session-archive/<proj>/`, keeping relative paths.
- Archived sessions disappear from `/resume` and are out of reach of `cleanupPeriodDays`, whose sweep deletes transcripts under `projects/`.
- Archive view lists them with the same rows; restore moves the footprint back.
- Restore refuses if a file with the same path already exists.

### Pin

- `p` pins or unpins; allowed on live sessions and in the archive, since it never touches the transcript.
- Pinned rows come first in the list (then newest first) and carry a `★` in their own column; search keeps relevance order and only marks them.
- Stored as ids in `~/.claude/sessile/pins.json`, so a pin follows its session into the archive and back. `/resume` does not see pins.
- `delete-empty` skips pinned sessions; `delete` warns in its dialog and drops the pin with the session.

### Export

- `# title`, a header (session, directory, branch, model, prompts, resume command), then `## You` / `## Claude` sections.
- Consecutive assistant entries merge into one reply, followed by the tools it used with counts (`Bash ×2, Read`).
- File name: ASCII slug of the title (max 60 chars) plus the first 8 chars of the id; an existing file is overwritten. The mod builds the name and hands the CLI the full path (`--output-file`).

### Delete

- Permanent `rm` of the footprint.
- Confirmation dialog shows the title and the exact list of paths to remove (`--dry-run`); yes runs `delete --yes`.

### Delete empty (bulk)

- Selects sessions with 0 prompts that also hold no assistant message and no attachment (a session started by a slash command, a screenshot or `!` input counts 0 prompts and is kept once anything followed), shows the list and the count, deletes after one confirmation.

### Resume

- `r` on a session of this project switches the running Claude Code to it in place (`$.command.run` of `/resume <id>`); the pane stays open on the list, the resumed session now marked `◆`.
- `/resume <id>` reaches only the current project's sessions (for another it prints "not found" and resolves anyway), so `r` on another project's session copies `cd '<cwd>' && claude --resume <id>` and draws it in the notice, for a new terminal.
- Not offered on live sessions (the current one included) or in the archive; a session that went live after the list was drawn is refused at the press, since two processes would append to one transcript.

### Copy helpers

- Copy session id.

### Guards

- The current session and any live session: no delete, archive or rename (rename appends to a file another process writes).
- Every destructive action re-checks the guard right before running, not only when the list was drawn.
- Every action ends with a toast stating what happened, or the error with the path.

## Not in v1

- Key bindings inside `/resume` (the mod API cannot extend that picker).
- Token stats.

## Tests

- CLI: `cargo test` in `cli/`, unit tests plus end-to-end runs against fixture transcripts in a temp `CLAUDE_CONFIG_DIR`.
- Mod: `claude plugin test` against a stateful fake of the CLI.
- Live: `python3 tests/live/live_test.py` runs the real pane in Claude Code in tmux, over generated sessions in their own `CLAUDE_CONFIG_DIR`; it covers what the kit cannot (focus ring, Escape, dock and inline layout, speed, real file moves).
- Human: `docs/manual-tests.md`, tasks for a person learning the tool from the pane alone.
- Covered: title resolution order, prompt counting, junk filters; footprint collection (id-keyed paths only, never `memory/`); guards on current and live sessions; archive/restore round-trip and restore collision; delete removes exactly the listed paths; all-projects list and search; export content.
