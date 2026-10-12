import type { On } from 'claude-code'

import type { CliDetail, CliRow, Listing, SessionRow, View, ViewState } from '../types'
import { emptyView } from '../hooks/view'

export const CURRENT = '00000000-0000-4000-8000-000000000000'
export const ROOT = '/home/me/ai'
export const OTHER = '/work/other'
// Rows of the other project start here, so their ids never meet this project's.
export const OTHER_FROM = 900
const NOW = Date.UTC(2026, 9, 3, 12)

const EXOTIC = [
  '日本語のセッション名がとても長いのでカラムを超える',
  '🚀 deploy 🔥 hotfix 👩‍💻 review',
  'a\u001b[31mred\u001b[0m title with an escape\u0007',
  'café · zażółć gęślą jaźń · é',
  `-starts with a dash`,
  'x'.repeat(300),
  '[b]markup[/b] <i>tags</i> {braces}',
  '',
]

export function makeRow(i: number, cwd = ROOT): SessionRow {
  const id = `${String(i).padStart(8, '0')}-0000-4000-8000-000000000000`
  const exotic = EXOTIC[i % 11]
  return {
    cwd,
    resumeCommand: `cd '${cwd}' && claude --resume ${id}`,
    id,
    title: i === 0 ? 'this one' : exotic ?? `session ${i}`,
    titleSource: exotic === '' ? 'none' : i % 3 === 0 ? 'custom' : 'ai',
    updatedMs: NOW - i * 3_600_000,
    sizeBytes: i % 7 === 0 ? 900 : 50_000 + i,
    prompts: i % 9 === 4 ? 0 : i % 13,
    isLive: i < 2,
    isPinned: false,
    firstPrompt: exotic === '' ? 'first prompt of an untitled one' : null,
  }
}

export type Cli = {
  calls: string[][]
  live: SessionRow[]
  archive: SessionRow[]
  // The next call answers with this instead of its result.
  failNext: 'exit' | 'json' | 'partial' | 'missing' | 'usage' | null
  // While set, every call waits for it.
  gate: Promise<void> | null
  // The session Claude Code is in; /resume and /clear move it.
  current: string
  // Sessions another Claude Code runs too, besides this window.
  heldElsewhere: Set<string>
  // Ids queued by delete-when-empty, and how many sweeps ran.
  queued: string[]
  sweeps: number
  // Whether /clear moves the window to a fresh session.
  isClearStuck: boolean
  // Runs when the mod sleeps, before the sleep passes at once.
  onSleep: (() => void) | null
  // Panes the mod closed and toasts it showed, in order.
  closed: string[]
  toasts: string[]
  // The mod's view as last written: the test's window into its state.
  view: View
}

type Out = { code: number; out: unknown }

// A page whose last transcript could not be read: printed without it, then
// exit 1 with the error object.
function partial(out: unknown) {
  const page = out as { items: CliRow[] }
  const items = page.items.slice(0, -1)
  const path = '/x/locked.jsonl'
  const stdout = JSON.stringify({ ...page, items, partial: true, unreadable: [{ path, message: 'Permission denied' }] })
  const error = { error: { kind: 'io_error', message: '1 transcript(s) could not be read and are left out: Permission denied', context: { paths: [path] } } }
  return { exitCode: 1, stdout, stderr: JSON.stringify(error), isStdoutTruncated: false, isStderrTruncated: false }
}

// A row as the binary prints it.
export function toCli(row: SessionRow): CliRow {
  const out: CliRow = {
    id: row.id,
    title: row.title,
    title_source: row.titleSource,
    updated_at: new Date(row.updatedMs).toISOString(),
    created_at: null,
    size_bytes: row.sizeBytes,
    prompts: row.prompts,
    is_live: row.isLive,
    is_pinned: row.isPinned,
    first_prompt: row.firstPrompt,
    cwd: row.cwd,
    resume_command: row.resumeCommand,
    interactive: true,
    is_current: false,
  }
  if (row.score !== undefined) out.score = row.score
  if (row.matchedIn !== undefined) out.matched_in = row.matchedIn
  if (row.snippet !== undefined) out.snippet = row.snippet
  return out
}

export function toCliDetail(row: SessionRow, rest: Omit<CliDetail, keyof CliRow>): CliDetail {
  return { ...toCli(row), ...rest }
}

function fail(code: number, kind: string, message: string): Out {
  return { code, out: { error: { kind, message } } }
}

function refuse(message: string): Out {
  return fail(1, 'session_live', message)
}

function missing(id: string | undefined): Out {
  return fail(1, 'not_found', `session ${id} not found in projects`)
}

// The subcommand follows the global flags, which end with `--current <id>`
// and, on every call but a file export, --json.
export function subcommand(argv: string[]): string | undefined {
  return argv[firstArg(argv)]
}

function firstArg(argv: string[]): number {
  const at = argv.indexOf('--current') + 2
  return argv[at] === '--json' ? at + 1 : at
}

function option(argv: string[], name: string): string | undefined {
  return argv.find(a => a.startsWith(`--${name}=`))?.slice(name.length + 3)
}

function answer(cli: Cli, argv: string[]): Out {
  const sub = subcommand(argv)
  const first = argv[firstArg(argv) + 1]
  const current = argv[argv.indexOf('--current') + 1]
  if (sub === 'sweep') {
    cli.sweeps += 1
    return { code: 0, out: { targets: [], skipped: [], dropped: [], changed: false } }
  }
  const isArchived = argv.includes('--archived')
  // Lists cover --project unless --all; an id is looked up in --project only
  // when it is given, as the real binary does. Queued sessions with no prompt
  // are left out unless another window runs them.
  const isHidden = (r: SessionRow) => cli.queued.includes(r.id) && r.prompts === 0 && (r.id === current || !r.isLive)
  const listed = (rows: SessionRow[]) => rows.filter(r => (argv.includes('--all') || r.cwd === ROOT) && !isHidden(r))
  const reachable = (rows: SessionRow[]) => (argv.includes('--project') ? rows.filter(r => r.cwd === ROOT) : rows)
  const store = isArchived ? cli.archive : cli.live
  const find = (id: string | undefined) => reachable(store).find(r => r.id === id)
  if (sub === 'list') {
    const rows = listed(store).sort((a, b) => Number(b.isPinned) - Number(a.isPinned) || b.updatedMs - a.updatedMs)
    const limit = Number(option(argv, 'limit') ?? 50)
    return { code: 0, out: { items: rows.slice(0, limit).map(toCli), has_more: rows.length > limit, partial: false } }
  }
  if (sub === 'search') {
    const q = (argv[argv.indexOf('--') + 1] ?? '').toLowerCase()
    const hits = listed(store).filter(r => r.title.toLowerCase().includes(q))
    const items = hits.slice(0, 50).map(r => toCli({ ...r, score: 1, matchedIn: 'title', snippet: `…${r.title}…` }))
    return { code: 0, out: { items, has_more: hits.length > 50, partial: false } }
  }
  if (sub === 'delete-empty') {
    const empty = cli.live.filter(r => r.prompts === 0 && !r.isLive && r.cwd === ROOT)
    const isDry = argv.includes('--dry-run')
    const targets = empty.map(r => ({ ...toCli(r), paths: [`/x/${r.id}.jsonl`] }))
    if (isDry) return { code: 0, out: { targets, skipped: [], changed: false, requires_confirmation: empty.length > 0 } }
    if (empty.length > 0 && !argv.includes('--yes')) return fail(1, 'confirmation_required', 'needs --yes')
    cli.live = cli.live.filter(r => !empty.includes(r))
    return { code: 0, out: { targets, skipped: [], changed: empty.length > 0 } }
  }
  // A pin is keyed by the id alone: the real binary takes no --archived for it.
  const anywhere = reachable([...cli.live, ...cli.archive]).find(r => r.id === first)
  const row = sub === 'restore' ? reachable(cli.archive).find(r => r.id === first) : sub === 'pin' || sub === 'unpin' ? anywhere : find(first)
  // The current session is queued before Claude Code has written it.
  if (row === undefined && sub === 'delete-when-empty' && current !== undefined && first === current) {
    if (!cli.queued.includes(current)) cli.queued.push(current)
    return { code: 0, out: { id: current, queued: true, changed: true } }
  }
  if (row === undefined) return missing(first)
  if (sub === 'get') {
    const turns = (from: number, to: number) => Array.from({ length: Math.max(0, to - from + 1) }, (_, i) => `prompt ${from + i} of ${row.title}`)
    const n = row.prompts
    const out = {
      first_prompts: turns(1, Math.min(3, n)),
      last_prompts: turns(Math.max(1, n - 2), n),
      last_reply: n === 0 ? null : `the reply that ends ${row.title}, `.repeat(12),
      git_branch: null,
      model: null,
      version: null,
      path: '/x',
    }
    return { code: 0, out: toCliDetail(row, out) }
  }
  // Under --output-file the result goes to the file and stdout stays empty.
  if (sub === 'export') return { code: 0, out: undefined }
  if (sub === 'pin' || sub === 'unpin') {
    const isPinned = sub === 'pin'
    const swap = (rows: SessionRow[]) => rows.map(r => (r === row ? { ...r, isPinned } : r))
    cli.live = swap(cli.live)
    cli.archive = swap(cli.archive)
    return { code: 0, out: { id: row.id, pinned: isPinned, changed: row.isPinned !== isPinned } }
  }
  if (sub === 'delete-when-empty') {
    if (row.prompts > 0) return fail(1, 'conflict', `session ${row.id} has a conversation`)
    if (cli.heldElsewhere.has(row.id)) return refuse('another Claude Code process is running it')
    const changed = !cli.queued.includes(row.id)
    if (changed) cli.queued.push(row.id)
    return { code: 0, out: { id: row.id, queued: true, changed } }
  }
  // --leaving plans the delete of the caller's own session: only another
  // process holding it refuses.
  const isLeaving = argv.includes('--leaving') && argv.includes('--dry-run') && row.id === current
  if (isLeaving ? cli.heldElsewhere.has(row.id) : row.isLive || row.id === current) return refuse('session is live')
  if (sub === 'delete') {
    const targets = [{ id: row.id, paths: [`/x/${row.id}.jsonl`] }]
    if (argv.includes('--dry-run')) return { code: 0, out: { targets, changed: false, requires_confirmation: true } }
    if (!argv.includes('--yes')) return fail(1, 'confirmation_required', 'needs --yes')
    cli.live = cli.live.filter(r => r !== row)
    cli.archive = cli.archive.filter(r => r !== row)
    return { code: 0, out: { targets, changed: true } }
  }
  if (sub === 'archive') {
    cli.live = cli.live.filter(r => r !== row)
    cli.archive = [...cli.archive, row]
    return { code: 0, out: { id: row.id, moved: [], changed: true } }
  }
  if (sub === 'restore') {
    cli.archive = cli.archive.filter(r => r !== row)
    cli.live = [...cli.live, row]
    return { code: 0, out: { id: row.id, moved: [], changed: true } }
  }
  if (sub === 'rename') {
    const title = argv[argv.indexOf('--') + 1] ?? ''
    const renamed = { ...row, title, titleSource: 'custom' as const }
    cli.live = cli.live.map(r => (r === row ? renamed : r))
    return { code: 0, out: { id: row.id, title, changed: true } }
  }
  return fail(2, 'invalid_input', `unknown ${sub}`)
}

// A stateful stand-in for the sessile binary, answering the argv the mod
// builds. `otherCount` sessions belong to another project.
export function fakeCli(on: On, rowCount = 40, otherCount = 5): Cli {
  const other = Array.from({ length: otherCount }, (_, i) => makeRow(OTHER_FROM + i, OTHER))
  const cli: Cli = {
    calls: [],
    live: [...Array.from({ length: rowCount }, (_, i) => makeRow(i)), ...other],
    archive: [],
    failNext: null,
    gate: null,
    current: CURRENT,
    heldElsewhere: new Set(),
    queued: [],
    sweeps: 0,
    isClearStuck: false,
    onSleep: null,
    closed: [],
    toasts: [],
    view: emptyView,
  }
  // /clear as Claude Code runs it: the window moves to a fresh, empty
  // session, live from then on, and the one it left is no longer live.
  let cleared = 0
  on('command.run', { command: 'clear' }, () => {
    if (cli.isClearStuck) return { text: '' }
    cleared += 1
    const fresh = { ...makeRow(700 + cleared), title: '', titleSource: 'none' as const, prompts: 0, isLive: true }
    const left = cli.current
    cli.live = [fresh, ...cli.live.map(r => (r.id === left ? { ...r, isLive: false } : r))]
    cli.current = fresh.id
    return { text: '' }
  })
  // The pause before the delete passes at once; the sweep timer never ticks.
  on('clock.sleep', () => {
    cli.onSleep?.()
    return { value: undefined }
  })
  on('clock.every', () => ({ value: undefined }))
  // The mod keeps the rows in their own key; the test reads one merged view.
  on('state.set', (_, e, next) => {
    const { rows, searchHits } = cli.view
    if (e.key === 'listing') cli.view = { ...cli.view, ...(e.value as Listing) }
    if (e.key === 'view') cli.view = { ...(e.value as ViewState), rows, searchHits }
    return next(e)
  })
  on('ui.focus', () => ({}))
  on('ui.scroll', () => ({}))
  on('ui.close', (_, e) => {
    cli.closed.push(e.id)
    return { value: undefined }
  })
  on('ui.toast', (_, e) => {
    cli.toasts.push(e.text)
    return { value: undefined }
  })
  on('ui.copy', () => ({ value: { isCopied: true } }))
  on('session.root', () => ({ value: ROOT }))
  on('session.id', () => ({ value: cli.current }))
  on('process.run', async (_, e) => {
    const argv = [...e.argv]
    cli.calls.push(argv)
    if (cli.gate !== null) await cli.gate
    const fail = cli.failNext
    cli.failNext = null
    if (fail === 'missing') {
      const stderr = `sessile: $.process.run(${argv[0]}) failed to start: ENOENT`
      return { value: { exitCode: -1, stdout: '', stderr, isStdoutTruncated: false, isStderrTruncated: false } }
    }
    if (fail === 'usage') {
      const stderr = JSON.stringify({ error: { kind: 'invalid_input', message: "unexpected argument '--current' found" } })
      return { value: { exitCode: 2, stdout: '', stderr, isStdoutTruncated: false, isStderrTruncated: false } }
    }
    // An injected failure never performs the operation.
    const { code, out } = fail === 'exit' ? refuse('injected failure') : fail === 'json' ? { code: 0, out: null } : answer(cli, argv)
    const text = fail === 'json' ? '{"truncated' : out === undefined ? '' : JSON.stringify(out)
    if (fail === 'partial') return { value: partial(out) }
    const value = { exitCode: code, stdout: code === 0 ? text : '', stderr: code === 0 ? '' : text, isStdoutTruncated: false, isStderrTruncated: false }
    return { value }
  })
  return cli
}
