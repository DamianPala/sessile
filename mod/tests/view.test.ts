import { expect, test } from 'claude-code/testing'

import type { SessionRow } from '../types'
import {
  ago,
  back,
  cells,
  cellWidth,
  clampOffset,
  columnsFor,
  edgeStep,
  emptyView,
  exportFileName,
  fit,
  followCursor,
  fromCli,
  fuzzyScore,
  GLYPHS,
  headerLabel,
  highlight,
  isJunk,
  joinPath,
  keyWidth,
  listChrome,
  listPlan,
  marker,
  packLines,
  projectName,
  rowLabel,
  settleTarget,
  shortPath,
  visibleRows,
} from '../hooks/view'

const row = (over: Partial<SessionRow>): SessionRow => ({
  id: 'aaaaaaaa-0000-0000-0000-000000000000',
  title: 'title',
  titleSource: 'ai',
  updatedMs: 0,
  sizeBytes: 100_000,
  prompts: 5,
  isLive: false,
  isPinned: false,
  firstPrompt: '',
  cwd: '/home/me/ai',
  resumeCommand: 'claude --resume x',
  ...over,
})

test('fuzzy matches subsequences and ranks word starts higher', () => {
  expect(fuzzyScore('gra', 'Upgrade Postgres to 17')).not.toBeNull()
  expect(fuzzyScore('xyz', 'Upgrade Postgres')).toBeNull()
  const start = fuzzyScore('mer', 'merge-message') ?? 0
  const middle = fuzzyScore('mer', 'summery') ?? 0
  expect(start).toBeGreaterThan(middle)
})

test('junk: few prompts, tiny files or no title', () => {
  expect(isJunk(row({ prompts: 1 }))).toBe(true)
  expect(isJunk(row({ sizeBytes: 1000 }))).toBe(true)
  expect(isJunk(row({ titleSource: 'none' }))).toBe(true)
  expect(isJunk(row({}))).toBe(false)
})

test('visibleRows filters by query and junk, keeps full-text hits as given', () => {
  const rows = [
    row({ id: '1', title: 'merge-message', updatedMs: 2 }),
    row({ id: '2', title: 'tagged release', updatedMs: 3 }),
    row({ id: '3', title: 'empty', prompts: 0, updatedMs: 1 }),
  ]
  const base = { ...emptyView, rows }
  expect(visibleRows(base).map(r => r.id)).toEqual(['1', '2', '3'])
  expect(visibleRows({ ...base, query: 'merge' }).map(r => r.id)).toEqual(['1'])
  expect(visibleRows({ ...base, isJunkOnly: true }).map(r => r.id)).toEqual(['3'])
  const hits = [row({ id: '2', title: 'tagged release' })]
  expect(visibleRows({ ...base, query: 'zzz', searchHits: hits }).map(r => r.id)).toEqual(['2'])
})

test('window offset clamps and follows the cursor off the edges', () => {
  expect(clampOffset(-5, 100, 10)).toBe(0)
  expect(clampOffset(95, 100, 10)).toBe(90)
  expect(clampOffset(3, 5, 10)).toBe(0)
  expect(followCursor(0, 9, 100, 10)).toBe(0)
  expect(followCursor(0, 10, 100, 10)).toBe(1)
  expect(followCursor(10, 10, 100, 10)).toBe(10)
  expect(followCursor(10, 9, 100, 10)).toBe(9)
  expect(followCursor(10, 14, 100, 10)).toBe(10)
  expect(followCursor(90, 99, 100, 10)).toBe(90)
  expect(followCursor(0, 50, 100, 10)).toBe(41)
})

test('edgeStep: from a row ▼/▲ slides one, from elsewhere takes the nearest row', () => {
  const rows = Array.from({ length: 30 }, (_, i) => row({ id: String(i) }))
  expect(edgeStep('row:14', 'down', rows, 5, 10)).toEqual({ offset: 6, id: '15' })
  expect(edgeStep('row:5', 'up', rows, 5, 10)).toEqual({ offset: 4, id: '4' })
  // A stale focus key (key repeat) still slides from the drawn window.
  expect(edgeStep('row:13', 'down', rows, 6, 10)).toEqual({ offset: 7, id: '16' })
  expect(edgeStep('k:r', 'down', rows, 5, 10)).toEqual({ offset: 5, id: '14' })
  expect(edgeStep('query', 'up', rows, 5, 10)).toEqual({ offset: 5, id: '5' })
  expect(edgeStep('row:0', 'up', rows, 0, 10)).toBeNull()
  expect(edgeStep('row:3', 'row:4', rows, 0, 10)).toBeNull()
})

test('settleTarget keeps a drawn cursor, else the first drawn row', () => {
  const rows = Array.from({ length: 30 }, (_, i) => row({ id: String(i) }))
  expect(settleTarget('7', rows, 5, 10)).toBe('7')
  expect(settleTarget('2', rows, 5, 10)).toBe('5')
  expect(settleTarget(null, rows, 5, 10)).toBe('5')
  expect(settleTarget(null, [], 0, 10)).toBeNull()
})

test('projectName takes the last segment of a POSIX or Windows path', () => {
  expect(projectName('/home/me/ai/lab/')).toBe('lab')
  expect(projectName('C:\\Users\\Ann\\proj')).toBe('proj')
  expect(projectName(null)).toBe('—')
})

test('fit pads and cuts by terminal cells and strips control characters', () => {
  const cells = (s: string) => Array.from(s).reduce((n, ch) => n + cellWidth(ch), 0)
  for (const title of ['zażółć gęślą jaźń', '日本語のセッション名がとても長い', '🚀 deploy 🔥 fix', 'é café', 'x'.repeat(50)]) {
    for (const width of [5, 10, 17, 30]) expect(cells(fit(title, width))).toBe(width)
  }
  expect(fit('a\u001b[31mred\u0007', 20)).not.toMatch(/[\u0000-\u001f]/)
  // A wide char that would straddle the ellipsis is dropped, the gap padded.
  expect(fit('日本語', 4)).toBe('日 …')
})

test('ago: minutes, hours, days, then dates', () => {
  const now = Date.UTC(2026, 9, 3, 12)
  expect(ago(now - 5 * 60_000, now)).toBe('5m')
  expect(ago(now - 5 * 3_600_000, now)).toBe('5h')
  expect(ago(now - 3 * 86_400_000, now)).toBe('3d')
  expect(ago(Date.UTC(2026, 8, 20, 12), now)).toBe('Sep 20')
  expect(ago(Date.UTC(2025, 8, 20, 12), now)).toBe('2025-09-20')
})

test('row label and header share the column layout', () => {
  const cols = columnsFor(120)
  const label = rowLabel(row({ title: 'x'.repeat(300) }), 0, cols)
  expect(label.length).toBe(headerLabel(cols).length)
  expect(label.length).toBeLessThanOrEqual(117)
  expect(columnsFor(80).showId).toBe(false)
  // Every project: a project column in front, the same total width.
  for (const width of [40, 80, 120]) {
    const all = columnsFor(width, true)
    const wide = rowLabel(row({ title: 'x'.repeat(300), cwd: '/a/very-long-project-name' }), 0, all)
    expect(wide.length).toBe(headerLabel(all).length)
    expect(wide.length).toBeLessThanOrEqual(width - 3)
    expect(headerLabel(all).startsWith('PROJECT')).toBe(true)
  }
})

test('markers: this session, live, renamed', () => {
  expect(marker(row({ id: 'me', isLive: true }), 'me').glyph).toBe('◆')
  expect(marker(row({ isLive: true }), 'me').glyph).toBe('●')
  expect(marker(row({ titleSource: 'custom' }), 'me').glyph).toBe('✎')
  expect(marker(row({}), 'me').glyph).toBe(' ')
})

test('every glyph is one cell and no two share a shape', () => {
  const glyphs = Object.values(GLYPHS).map(g => g.glyph)
  expect(new Set(glyphs).size).toBe(glyphs.length)
  for (const g of glyphs) expect(cells(g)).toBe(1)
})

test('highlight splits around the first query word', () => {
  expect(highlight('Backups on VM 121 behind a proxy', 'vm 121')).toEqual(['Backups on ', 'VM', ' 121 behind a proxy'])
  expect(highlight('nothing', 'zzz')).toEqual(['nothing', '', ''])
})

test('buttons pack into as many lines as the width needs, and the list plans for the widest labels', () => {
  expect(keyWidth('v', 'archive')).toBe('v [ archive ]'.length)
  expect(keyWidth('esc', 'back')).toBe('esc [ back ]'.length)
  // 10 + 2 + 10 fits 22; the third starts a line; one too wide gets its own.
  expect(packLines([10, 10, 10], 22)).toEqual([[0, 1], [2]])
  expect(packLines([10, 30, 5], 22)).toEqual([[0], [1], [2]])
  // A docked pane is ~58 columns: footer in three lines, row actions in two.
  expect(listChrome(58)).toBe(7 + 2 + 3)
  expect(listChrome(80)).toBe(7 + 2 + 2)
  // The footer's widest labels need 124 columns for one line.
  expect(listChrome(120)).toBe(7 + 1 + 2)
  expect(listChrome(200)).toBe(7 + 1 + 1)
})

test('a low pane gets the compact list instead of one squeezed row', () => {
  expect(listPlan(60, 200, 1)).toEqual({ size: 51, isCompact: false })
  // The full layout while it still shows four rows, the compact one below.
  expect(listPlan(11 + 4, 80, 1)).toEqual({ size: 4, isCompact: false })
  expect(listPlan(11 + 3, 80, 1)).toEqual({ size: 10, isCompact: true })
  // An inline pane: 8 body rows on a 30-row terminal, 6 on a 24-row one.
  expect(listPlan(8, 100, 1)).toEqual({ size: 4, isCompact: true })
  expect(listPlan(6, 80, 1)).toEqual({ size: 2, isCompact: true })
  // Full-text hits take two lines each; one always shows.
  expect(listPlan(6, 80, 2)).toEqual({ size: 1, isCompact: true })
  expect(listPlan(3, 80, 1).size).toBe(1)
})

test('shortPath and back', () => {
  expect(shortPath('/home/me/.claude/x')).toBe('~/.claude/x')
  expect(shortPath('C:\\Users\\Ann\\proj\\x.md')).toBe('~\\proj\\x.md')
  const r = row({})
  expect(back({ kind: 'rename', row: r, detail: null })).toEqual({ kind: 'list' })
  expect(back({ kind: 'list' })).toEqual({ kind: 'list' })
})

test('export file names: ASCII words, dashes, an id prefix, a fallback', () => {
  const id = '0a1b2c3d-7c0f-4b5a-9e1d-1234567890ab'
  expect(exportFileName('Fix: the  CI (again)!', id)).toBe('fix-the-ci-again-0a1b2c3d.md')
  expect(exportFileName('日本語', id)).toBe('session-0a1b2c3d.md')
  expect(exportFileName('', id)).toBe('session-0a1b2c3d.md')
  expect(exportFileName('x'.repeat(200), id)).toBe(`${'x'.repeat(60)}-0a1b2c3d.md`)
  expect(joinPath('/home/me/ai/', 'a.md')).toBe('/home/me/ai/a.md')
  expect(joinPath('~/exports', 'a.md')).toBe('~/exports/a.md')
})

test('fromCli maps the snake_case row and reads the RFC 3339 time', () => {
  const r = fromCli({
    id: 'a',
    title: 't',
    title_source: 'ai',
    updated_at: '2026-10-03T14:10:43.207Z',
    created_at: '2026-10-03T14:02:11.518Z',
    size_bytes: 10,
    prompts: 2,
    is_live: false,
    is_pinned: true,
    first_prompt: null,
    cwd: '/x',
    resume_command: 'claude --resume a',
    interactive: true,
    is_current: false,
    matched_in: 'prompt',
    snippet: 's',
  })
  expect(r.updatedMs).toBe(1_791_036_643_207)
  expect(r.isPinned).toBe(true)
  expect(r.matchedIn).toBe('prompt')
  expect(r.score).toBeUndefined()
})
