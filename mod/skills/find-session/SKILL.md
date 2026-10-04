---
name: find-session
description: Find, read and manage past Claude Code sessions with the sessile CLI, to answer which session worked on a topic, where work stopped, or what was decided and where. Use when the user asks about earlier conversations or sessions, wants to resume one, needs a fact that lives only in a past transcript, or wants sessions renamed, archived, cleaned up or deleted. DO NOT TRIGGER for recalling what was said earlier in the current conversation.
metadata:
  version: "0.2.0"
---

# Find a session

`sessile` reads the local Claude Code transcripts. `sessile <command> --help` holds the full contract, and `sessile schema <command>` the same as JSON, with the shape of every result. This skill covers what they do not tell you.

## Search

Always pass `--all`. A session belongs to the directory it was started in, and people often start sessions in a parent workspace rather than the project directory. A project directory may hold only headless runs (reviews, smoke tests).

Pick the fields by the question:
- **Which session worked on X**: `--in title,prompt`. Prompts are what the user typed, so a hit there means the topic was worked on. A title names the first problem of a session and can be unrelated to the rest, so a title hit is a lead, not proof, and a missing one means nothing. Hits only in assistant replies are mostly passing mentions here.
- **What was decided, said or found about X**: search all fields (drop `--in`). Decisions and findings are usually written in assistant replies. An assistant hit in a session whose title names the project is a strong candidate.

Queries:
- Words match as substrings, and all words of a query must come up together in one title, one prompt or one reply. Each added word therefore cuts hits: use one or two distinctive terms per query, and put a further term (the project name) into its own variant instead of appending it. A single common word ("typo", "config") returns noise. Quotes are not phrase syntax.
- Distinctive terms: a feature, file or command name, a path (`lab/projects/<name>`), an error message. For a short tool name ("hop" also finds "Workshop"), search its path or subcommands instead.
- A term that starts with `-` goes last, after `--`: `sessile search --all --json -- "--all"`, `sessile excerpts <id> --plain -- "--all"`.
- Search is lexical, so you supply the semantics. The user's words are often not the session's words. Before the first search, write 3–5 variants and run each as its own command, in parallel:
  - the user's words,
  - synonyms and the project's own name for the thing,
  - how the answer itself would be phrased, which is what the assistant wrote. For a concept, that is its concrete form, e.g. "429", "too many requests" or "backoff" for "rate limiting". For a bug, it is the symptom, the cause or the error text,
  - each of those in the user's other working language, since the question and the session may differ.

  Use the same variants for `excerpts`.
- `--min-prompts 3` drops one-shot runs. Lower it when a short focused session could be the answer. `interactive: null` marks a transcript from an older Claude Code that may still be a real session: filter by prompt count, not by `interactive`.
- `is_current: true` is this conversation: skip it.

Rows are long, so project them. One plain command per variant: a shell loop or variable is refused by permission checks.

```bash
sessile search "<variant>" --all --min-prompts 3 --limit 10 --json \
  | jq -r '.items[] | [.updated_at[:10], .prompts, .matched_in, .is_current, .id, .title] | @tsv'
```

Add `--in title,prompt` when the question is which session worked on X. A session that turns up for several variants is the strongest candidate. A long session that turns up for nearly every query (an orchestration or "development sessions" log) is usually a passing mention.

**When did we last …**: results are ranked by match, not by time. Collect the candidates of all variants, take the newest by `updated_at` and confirm them with `excerpts`. Turns carry no dates: give the session's `created_at` to `updated_at` range and say that the day of a single turn is not known.

## Read

Never export a whole session to read it: exports run to megabytes. Find the turns, then read only those.

- `sessile get <id> --json | jq '{title, created_at, updated_at, prompts, first_prompts, last_prompts}'` shows what a session was about, not how it ended.
- `sessile excerpts <id> "<term>" --plain` lists the turns where a term comes up, one per line: turn, role (`prompt` or `assistant`), snippet. It stops at 50 (`--limit`), with a note on stderr.
- A snippet only locates a turn and is often cut before the useful part. Read the hit turn itself with `sessile export <id> --turns <n>` before trying other words. One turn can run to many KB, so search inside it with `sessile export <id> --turns <n> | rg -n -C8 '<term>'` rather than cutting it with `head`.
- **Where a topic stopped**: read the turn of the last excerpt hit and one or two after it. Don't use `--last-turns` for this, since the end of a session is often another topic. The last turn on a topic may also be follow-up work (docs, notes) after the last real change, so report both if they differ.
- **What was decided or found**: hits with role `assistant`. The first hit is usually where it was found or first stated, and the last hit holds the final version if the session revised it. Read both when they differ.
- **Main session**, when several sessions worked on the topic: the one with the most excerpt hits for it, not the most prompts in total; when both reach the limit, compare the hits with role `prompt`. A short focused session that answers the question is enough on its own.
- Before saying "this is the session", read the turn. A title or a search hit is not enough. Once a turn you read answers the question, stop searching.

## Answer

Give the id, title, dates (`created_at` to `updated_at`) and the row's `resume_command`. Cite the turn numbers you read and say whether you confirmed the content or judged from metadata. When several sessions match, name the main one and list the rest with one line each. Note that a later session may have changed the state you report. For a decision, quote the final version and mention earlier ones if the session revised it.

## Change sessions

Stay read-only unless the user asks for the change. `rename`, `pin`, `unpin`, `archive`, `restore`, `delete` and `delete-empty` change the user's sessions; `sessile <command> --help` has each contract. The CLI refuses the current session and any session a Claude Code process is running; say so instead of working around it.

Cleaning up: list the candidates (`sessile list --project <dir> --include-headless --json`, or `--all`), show them with title, date, prompts and size, and ask which to remove. Prefer `archive`, which `restore` undoes; `delete` and `delete-empty` are permanent, so run their `--dry-run` first and show what it lists. Never remove or move files under the Claude config directory by hand: a session is more files than its transcript, and pins and the archive would be bypassed.
