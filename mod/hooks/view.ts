import type { CliDetail, CliRow, Listing, Mode, SessionDetail, SessionRow, View, ViewState } from '../types'

export const JUNK_MAX_PROMPTS = 1
export const JUNK_MAX_BYTES = 20 * 1024

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

export const emptyView: View = {
  rows: [],
  currentId: '',
  query: '',
  searchHits: null,
  isArchiveView: false,
  isAllProjects: false,
  isJunkOnly: false,
  offset: 0,
  cursorId: null,
  focusKey: null,
  draft: '',
  mode: { kind: 'list' },
  notice: null,
  isBandOpen: false,
}

export const emptyListing: Listing = { rows: [], searchHits: null }

// Drops the listing, also one a state saved before the split still carries.
export function withoutListing(v: ViewState & Partial<Listing>): ViewState {
  const { rows: _rows, searchHits: _hits, ...rest } = v
  return rest
}

export function isJunk(row: SessionRow): boolean {
  return (
    row.prompts <= JUNK_MAX_PROMPTS ||
    row.sizeBytes < JUNK_MAX_BYTES ||
    row.titleSource === 'none'
  )
}

// Subsequence match, scored so contiguous runs and word starts rank first.
export function fuzzyScore(needle: string, hay: string): number | null {
  const n = needle.toLowerCase()
  const h = hay.toLowerCase()
  let score = 0
  let run = 0
  let from = 0
  for (const ch of n) {
    if (ch === ' ') continue
    const at = h.indexOf(ch, from)
    if (at < 0) return null
    run = at === from ? run + 1 : 0
    const isWordStart = at === 0 || /[\s\-_/.]/.test(h[at - 1] ?? '')
    score += 1 + run * 2 + (isWordStart ? 3 : 0)
    from = at + 1
  }
  return score
}

export function visibleRows(view: View): SessionRow[] {
  const base = view.searchHits ?? view.rows
  const junked = view.isJunkOnly ? base.filter(isJunk) : base
  const q = view.query.trim()
  if (q === '' || view.searchHits !== null) return junked
  return junked
    .map(row => ({ row, score: fuzzyScore(q, `${row.title} ${row.id}`) }))
    .filter((hit): hit is { row: SessionRow; score: number } => hit.score !== null)
    .sort((a, b) => b.score - a.score || b.row.updatedMs - a.row.updatedMs)
    .map(hit => hit.row)
}

export function clampOffset(offset: number, total: number, room: number): number {
  return Math.min(Math.max(0, offset), Math.max(0, total - room))
}

// Scrolls only as far as needed to keep the cursor row inside the window.
export function followCursor(offset: number, index: number, total: number, room: number): number {
  if (index < 0) return offset
  if (index < offset) return clampOffset(index, total, room)
  if (index >= offset + room) return clampOffset(index - room + 1, total, room)
  return offset
}

// A person's move landing on ▲/▼. From a row it means "one more row": the
// window slides one and the ring takes the row that came into view. From the
// filter or the buttons below it means "the nearest drawn row", no slide.
export function edgeStep(
  focusKey: string | null,
  element: string | null,
  rows: SessionRow[],
  drawn: number,
  room: number,
): { offset: number; id: string } | null {
  if (element !== 'down' && element !== 'up') return null
  const isDown = element === 'down'
  const isFromRow = focusKey?.startsWith('row:') === true
  const slide = isFromRow ? (isDown ? 1 : -1) : 0
  const index = isDown ? Math.min(drawn + room, rows.length) - 1 + slide : drawn + slide
  const target = rows[index]
  if (index < 0 || target === undefined) return null
  return { offset: clampOffset(drawn + slide, rows.length, room), id: target.id }
}

// Where the ring rests after the window or the rows changed under it: the
// cursor row while it is still drawn, else the window's first row.
export function settleTarget(cursorId: string | null, rows: SessionRow[], drawn: number, room: number): string | null {
  const index = rows.findIndex(r => r.id === cursorId)
  if (index >= drawn && index < drawn + room) return cursorId
  return rows[drawn]?.id ?? null
}

// The ring on one of these still means "this row": the row actions stay drawn.
export const ROW_ACTION_KEYS = ['k:r', 'k:t', 'k:a', 'k:d', 'k:p', 'k:m', 'k:i']

// The labels each key can show, every state's: the drawing picks one, and the
// list's height is planned for the widest, so a toggle never moves the rows.
export const FOOTER_KEYS = {
  // "show": a place to go, apart from the row's `a archive`, a thing to do.
  v: ['show archive', 'show sessions'],
  w: ['all projects', 'this project'],
  x: ['junk', 'junk off'],
  e: ['delete empty'],
  c: ['clear'],
  f: ['filter'],
  g: ['reload'],
  q: ['close'],
} as const
export const ROW_KEYS = {
  r: ['resume'],
  t: ['rename'],
  a: ['archive', 'restore'],
  d: ['delete'],
  p: ['pin', 'unpin'],
  m: ['export md'],
  i: ['copy id'],
} as const
// The compact list's labels when a line of keys is wider than the pane.
const SHORT_LABELS: Record<string, string> = {
  'show archive': 'archive',
  'show sessions': 'sessions',
  'all projects': 'all',
  'this project': 'this',
  'delete empty': 'empty',
  'export md': 'md',
  'copy id': 'id',
}

export function shortLabel(label: string): string {
  return SHORT_LABELS[label] ?? label
}

export const LIVE_NOTE = 'live session: read-only'
const KEY_GAP = 2

export function cells(text: string): number {
  return Array.from(text).reduce((n, ch) => n + cellWidth(ch), 0)
}

// A cap beside its bracketed button: `k [ label ]`.
export function keyWidth(key: string, label: string): number {
  return cells(key) + 1 + cells(label) + 4
}

// Lines filled left to right in the given order, as item indexes; an item
// wider than a line gets one to itself and is clipped.
export function packLines(widths: number[], columns: number): number[][] {
  const lines: number[][] = []
  let used = 0
  widths.forEach((width, i) => {
    const line = lines[lines.length - 1]
    if (line !== undefined && used + KEY_GAP + width <= columns) {
      line.push(i)
      used += KEY_GAP + width
      return
    }
    lines.push([i])
    used = width
  })
  return lines
}

function widest(keys: Record<string, readonly string[]>): number[] {
  return Object.entries(keys).map(([key, labels]) => Math.max(...labels.map(label => keyWidth(key, label))))
}

// Lines of the list view that are not session rows: header, filter, column
// header, the two "more" lines, the legend and the notice, plus the row
// actions (or the one-line hint in their place) and the footer as many lines
// as they take.
export function listChrome(columns: number): number {
  const footer = packLines(widest(FOOTER_KEYS), columns).length
  const row = widest(ROW_KEYS)
  // A live row: the note, then pin, export and copy id.
  const live = [cells(LIVE_NOTE), ...row.slice(4)]
  const actions = Math.max(1, packLines(row, columns).length, packLines(live, columns).length)
  return 7 + actions + footer
}

// The compact list keeps four lines for itself: the header (or a notice in
// its place), the filter with "▲ more" at its end, one line of "▼ more" and
// row actions, one line of footer keys.
export const COMPACT_CHROME = 4
const FULL_LAYOUT_ROWS = 4

// How many sessions the list draws and in which layout. An inline pane gets
// about a quarter of the screen: the full chrome would leave it a row or none,
// so below four rows the list drops the column header, the legend and the
// reserved notice line, and clips the keys to one line each.
export function listPlan(bodyRows: number, columns: number, perRow: number): { size: number; isCompact: boolean } {
  const full = Math.floor((bodyRows - listChrome(columns)) / perRow)
  if (full >= FULL_LAYOUT_ROWS) return { size: full, isCompact: false }
  return { size: Math.max(1, Math.floor((bodyRows - COMPACT_CHROME) / perRow)), isCompact: true }
}

// Letter keys reach the buttons only while the pane holds the keys and the
// filter does not: a focused Input takes every printable key.
export function isArmed(v: View, isFocused: boolean): boolean {
  return isFocused && v.focusKey !== null && v.focusKey !== 'query'
}

export function hasFilter(v: View): boolean {
  return v.searchHits !== null || v.query !== ''
}

export function isOnRows(key: string | null): boolean {
  return key === null || key === 'up' || key === 'down' || key.startsWith('row:') || ROW_ACTION_KEYS.includes(key)
}

// The window as drawn. While the ring is on the cursor row or its actions the
// window keeps that row in view, so a smaller pane (a resize, a dock) never
// leaves the ring on a row it no longer draws.
export function windowOffset(v: View, rows: SessionRow[], room: number): number {
  const offset = clampOffset(v.offset, rows.length, room)
  const key = v.focusKey ?? ''
  if (!key.startsWith('row:') && !ROW_ACTION_KEYS.includes(key)) return offset
  return followCursor(offset, rows.findIndex(r => r.id === v.cursorId), rows.length, room)
}

const ALWAYS_DRAWN = ['query', 'k:f', 'k:x', 'k:v', 'k:w', 'k:g', 'k:q']

// Whether the list view of `v` draws the element under `key`, mirroring
// drawList: the ring may only rest on what is drawn.
export function isDrawnInList(v: View, key: string, room: number): boolean {
  if (ALWAYS_DRAWN.includes(key)) return true
  if (key === 'k:c') return hasFilter(v)
  if (key === 'k:u') return v.notice?.undo !== undefined
  if (key === 'k:e') return !v.isArchiveView && !v.isAllProjects
  const rows = visibleRows(v)
  const drawn = windowOffset(v, rows, room)
  const shown = rows.slice(drawn, drawn + room)
  if (key === 'up') return drawn > 0
  if (key === 'down') return drawn + shown.length < rows.length
  if (key.startsWith('row:')) return shown.some(r => `row:${r.id}` === key)
  const cursor = shown.find(r => r.id === v.cursorId)
  if (cursor === undefined) return false
  if (key === 'k:i' || key === 'k:m' || key === 'k:p') return true
  if (key === 'k:r' || key === 'k:t') return !cursor.isLive && !v.isArchiveView
  return (key === 'k:a' || key === 'k:d') && !cursor.isLive
}

export function ago(ms: number, now: number): string {
  const min = Math.max(0, Math.floor((now - ms) / 60000))
  if (min < 60) return `${min}m`
  const hours = Math.floor(min / 60)
  if (hours < 24) return `${hours}h`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days}d`
  const date = new Date(ms)
  const month = MONTHS[date.getMonth()] ?? ''
  if (date.getFullYear() === new Date(now).getFullYear()) return `${month} ${date.getDate()}`
  return date.toISOString().slice(0, 10)
}

export function bytes(n: number): string {
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}K`
  return `${(n / 1024 / 1024).toFixed(1)}M`
}

const WIDE: Array<[number, number]> = [
  [0x1100, 0x115f], [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf], [0x4e00, 0x9fff],
  [0xa000, 0xa4cf], [0xac00, 0xd7a3], [0xf900, 0xfaff], [0xfe30, 0xfe4f], [0xff00, 0xff60],
  [0xffe0, 0xffe6], [0x1f300, 0x1f64f], [0x1f680, 0x1f6ff], [0x1f900, 0x1f9ff], [0x20000, 0x3fffd],
]
const ZERO_WIDTH = /[̀-ͯ​-‏⃐-⃿︀-️]/

// Terminal cells, not UTF-16 units: CJK and most emoji take two, combining
// marks, joiners and variation selectors none.
export function cellWidth(ch: string): number {
  if (ZERO_WIDTH.test(ch)) return 0
  const code = ch.codePointAt(0) ?? 0
  return WIDE.some(([from, to]) => code >= from && code <= to) ? 2 : 1
}

// Control characters would let a title move the cursor or recolour the screen,
// and the engine refuses a tree that carries one.
export function stripControl(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f-\u009f]/g, ' ')
}

export function clean(text: string): string {
  return stripControl(text).replace(/\s+/g, ' ').trim()
}

export function fromCli(raw: CliRow): SessionRow {
  const row: SessionRow = {
    id: raw.id,
    title: raw.title,
    titleSource: raw.title_source,
    updatedMs: Date.parse(raw.updated_at),
    sizeBytes: raw.size_bytes,
    prompts: raw.prompts,
    isLive: raw.is_live,
    isPinned: raw.is_pinned,
    firstPrompt: raw.first_prompt,
    cwd: raw.cwd,
    resumeCommand: raw.resume_command,
  }
  if (raw.score !== undefined) row.score = raw.score
  if (raw.matched_in !== undefined) row.matchedIn = raw.matched_in
  if (raw.snippet !== undefined) row.snippet = raw.snippet
  return row
}

export function detailFromCli(raw: CliDetail): SessionDetail {
  return {
    ...fromCli(raw),
    firstPrompts: raw.first_prompts,
    lastPrompts: raw.last_prompts,
    gitBranch: raw.git_branch,
    model: raw.model,
    version: raw.version,
    path: raw.path,
  }
}

const EXPORT_NAME_KEEP = 60

// `<title>-<id prefix>.md`, ASCII only so the name is valid on every system.
export function exportFileName(title: string, id: string): string {
  let stem = ''
  for (const ch of title.toLowerCase()) {
    if (/^[a-z0-9]$/.test(ch)) stem += ch
    else if (stem !== '' && !stem.endsWith('-')) stem += '-'
    if (stem.length >= EXPORT_NAME_KEEP) break
  }
  stem = stem.replace(/-+$/, '')
  return `${stem === '' ? 'session' : stem}-${id.slice(0, 8)}.md`
}

export function joinPath(dir: string, name: string): string {
  return `${dir.replace(/[/\\]+$/, '')}/${name}`
}

// Session text comes from transcripts: the engine refuses a whole tree with a
// control character in it, so every row is cleaned once, where it arrives.
export function cleanRow<T extends SessionRow>(row: T): T {
  const out: T = { ...row, title: clean(row.title) }
  if (row.firstPrompt !== null) out.firstPrompt = clean(row.firstPrompt)
  out.cwd = typeof row.cwd === 'string' ? clean(row.cwd) : null
  if (row.snippet !== undefined) out.snippet = clean(row.snippet)
  return out
}

export function cleanDetail(d: SessionDetail): SessionDetail {
  return {
    ...cleanRow(d),
    firstPrompts: d.firstPrompts.map(clean),
    lastPrompts: d.lastPrompts.map(clean),
    gitBranch: d.gitBranch === null ? null : clean(d.gitBranch),
    model: d.model === null ? null : clean(d.model),
  }
}

export function fit(text: string, width: number): string {
  let out = ''
  let used = 0
  const chars = Array.from(clean(text))
  const total = chars.reduce((sum, ch) => sum + cellWidth(ch), 0)
  const limit = total <= width ? width : width - 1
  for (const ch of chars) {
    const w = cellWidth(ch)
    if (used + w > limit) break
    out += ch
    used += w
  }
  return total <= width ? out + ' '.repeat(width - used) : `${out}${' '.repeat(Math.max(0, limit - used))}…`
}

// `project` is the width of the project column, 0 when only one project is listed.
export type Columns = { title: number; project: number; showId: boolean; showCounts: boolean }

const ID_WIDTH = 10
// The pin, the marker and a space, drawn before the row label.
export const GUTTER = 3

// Narrow panes drop the id, then prompts and size, before the title gets too
// short to tell sessions apart.
export function columnsFor(width: number, showProject = false): Columns {
  const showId = width >= 100
  const showCounts = width >= 60
  const meta = 8 + (showCounts ? 16 : 0) + (showId ? ID_WIDTH : 0)
  const project = showProject ? Math.min(18, Math.max(8, Math.floor(width / 6))) : 0
  const title = Math.max(8, width - GUTTER - meta - 1 - (project > 0 ? project + 1 : 0))
  return { title, project, showId, showCounts }
}

// The last segment of the directory, POSIX or Windows.
// The project dir Claude Code files a session under: every non-alphanumeric
// UTF-16 unit of the cwd becomes `-` (as the CLI's slug_for).
export function projectSlug(dir: string): string {
  return dir.replace(/[\\/]+$/, '').replace(/[^A-Za-z0-9]/g, '-')
}

export function projectName(cwd: string | null): string {
  return cwd?.split(/[\\/]/).filter(Boolean).pop() ?? '—'
}

function metaCells(updated: string, prompts: string, size: string, id: string, cols: Columns) {
  const counts = cols.showCounts ? `${prompts.padStart(8)}${size.padStart(8)}` : ''
  const cells = `${updated.padStart(8)}${counts}`
  return cols.showId ? `${cells}  ${id.padEnd(8)}` : cells
}

function projectCell(name: string, cols: Columns): string {
  return cols.project > 0 ? `${fit(name, cols.project)} ` : ''
}

export function headerLabel(cols: Columns): string {
  return `${projectCell('PROJECT', cols)}${fit('TITLE', cols.title)} ${metaCells('UPDATED', 'PROMPTS', 'SIZE', 'ID', cols)}`
}

export function rowLabel(row: SessionRow, now: number, cols: Columns): string {
  const title = fit(row.title || row.firstPrompt || row.id, cols.title)
  const meta = metaCells(ago(row.updatedMs, now), String(row.prompts), bytes(row.sizeBytes), row.id.slice(0, 8), cols)
  return `${projectCell(projectName(row.cwd), cols)}${title} ${meta}`
}

export type Glyph = { glyph: string; label: string; color?: string; dim?: boolean }

// Told apart by shape alone, so a light theme or no colour still reads. The
// star is the pin because people know it as "keep at the top".
export const GLYPHS = {
  pinned: { glyph: '★', label: 'pinned', color: 'yellow' },
  current: { glyph: '◆', label: 'this', color: 'cyan' },
  live: { glyph: '●', label: 'live', color: 'green' },
  named: { glyph: '✎', label: 'named', dim: true },
} as const satisfies Record<string, Glyph>

const NO_MARK: Glyph = { glyph: ' ', label: '' }

// One marker column, so the first that applies wins: the current session is
// always live, and a live named one still shows its title.
export function marker(row: SessionRow, currentId: string): Glyph {
  if (row.id === currentId) return GLYPHS.current
  if (row.isLive) return GLYPHS.live
  if (row.titleSource === 'custom') return GLYPHS.named
  return NO_MARK
}

export function shortPath(path: string): string {
  return path.replace(/^(\/(home|Users)\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)/, '~')
}

// Splits a snippet around the first case-insensitive hit of any query word.
export function highlight(snippet: string, query: string): [string, string, string] {
  const lower = snippet.toLowerCase()
  for (const word of query.toLowerCase().split(/\s+/).filter(w => w.length > 1)) {
    const at = lower.indexOf(word)
    if (at >= 0) return [snippet.slice(0, at), snippet.slice(at, at + word.length), snippet.slice(at + word.length)]
  }
  return [snippet, '', '']
}

export function back(mode: Mode): Mode {
  if (mode.kind === 'rename' || mode.kind === 'confirm-delete') {
    return mode.detail === null ? { kind: 'list' } : { kind: 'detail', detail: mode.detail }
  }
  return { kind: 'list' }
}
