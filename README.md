# sessile

Manage your local Claude Code sessions without leaving Claude Code: browse, search, resume, rename, pin, archive, restore, export and delete them, in the current project or in all of them.

`/resume` can pick a session and rename it. sessile adds the rest: full-text search over prompts and replies, an archive that takes sessions out of `/resume` and brings them back, Markdown export, and deletion of one session with everything Claude Code stored for it.

It has three parts:

- **A pane inside Claude Code** (`/sessile`): a list beside the conversation, driven by the keyboard or the mouse.
- **A CLI** (`sessile`): the engine behind the pane, usable on its own and from scripts. JSON output, `--dry-run` and `--yes` on anything permanent.
- **An agent skill** (`sessile:find-session`): lets Claude answer "which session did we do X in", "where did we stop with Y", "what did we decide about Z" from your past transcripts.

Everything is local. sessile reads and changes files under your Claude Code config directory (`~/.claude`) and sends nothing anywhere.

## Status

Version 0.1.0, not released yet. Developed and used on Linux. macOS and Windows are built and tested in CI only; nobody has used the pane there yet.

## Install

The pane needs the CLI, so install both.

**1. The CLI** (needs a [Rust toolchain](https://rustup.rs)):

```bash
cargo install --locked --git https://github.com/DamianPala/sessile sessile
sessile --version
```

**2. The plugin**, inside Claude Code:

```text
/plugin marketplace add DamianPala/sessile
/plugin install sessile@sessile
```

Then `/sessile` opens the pane. If the pane says the CLI was not found, Claude Code does not see `sessile` on its `PATH`: set the full path in `/config` → sessile → `cliPath` (`which sessile` prints it).

Update both together: the pane and the CLI of one version belong to each other.

## The pane

`/sessile` (or `/sessions`) opens it. Type to filter by title, press `Enter` to search prompts and replies, `↓` to walk the rows. On a row:

| Key | Does |
|---|---|
| `Enter` | details: id, directory, branch, first and last prompts |
| `r` | resume (switches this conversation; for another project it copies the command for a new terminal) |
| `t` | rename |
| `p` | pin to the top |
| `a` | archive, or restore in the archive view; `u` undoes it |
| `m` | export as Markdown |
| `d` | delete for good, after a dialog that lists every file |
| `i` | copy the id |

Below the list: `v` archive view, `w` all projects, `x` junk only (at most one prompt, under 20 KB, or untitled), `e` delete empty sessions, `g` reload, `q` close. `Esc` steps back one level.

The session you are in and sessions another Claude Code is running cannot be renamed, archived or deleted.

## The CLI

```bash
sessile search "rate limit" --all            # every project, best match first
sessile list --limit 20                      # this project, newest first
sessile get <id>                             # one session in detail
sessile excerpts <id> "backoff"              # where a word comes up inside a session
sessile export <id> --turns 12-15            # those turns as Markdown
sessile archive <id>                         # out of /resume; sessile restore <id> undoes it
sessile delete <id> --dry-run                # what would be removed
```

Output is a table at a terminal and JSON elsewhere. `sessile --help`, `sessile <command> --help` and `sessile schema` describe everything; the full contract is in [cli/README.md](cli/README.md).

## What archive and delete touch

A session is more than its transcript. Both commands act on every file Claude Code keys by the session id (transcript, subagent and tool-result directory, file history, tasks, todos, debug log) and on nothing else. Archive moves them to `~/.claude/session-archive/`; delete removes them. Typed prompts also stay in Claude Code's `~/.claude/history.jsonl`, which sessile does not edit.

## Development

- `cli/`: Rust. `cargo test`, `cargo clippy --all-targets` in `cli/`.
- `mod/`: the Claude Code plugin (TypeScript hooks, the pane, the skill). `claude plugin validate mod`, `claude plugin test mod`.
- `tests/live/live_test.py`: drives the real pane in Claude Code inside tmux, over generated sessions.
- [SPEC.md](SPEC.md): design and behaviour.

## License

MIT, see [LICENSE](LICENSE).
