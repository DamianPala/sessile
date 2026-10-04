import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'
import type { SessionRow } from '../types'
import { subcommand, toCli, toCliDetail } from './fake-cli'

const NOW = Date.now()
const row = (i: number): SessionRow => ({
  id: `${String(i).padStart(8, '0')}-0000-4000-8000-000000000000`,
  title: i === 0 ? 'this one' : `session ${i}`,
  titleSource: i % 3 === 0 ? 'custom' : 'ai',
  updatedMs: NOW - i * 3_600_000,
  sizeBytes: 50_000 + i,
  prompts: i,
  isLive: i < 2,
  isPinned: false,
  firstPrompt: null,
  cwd: '/home/me/ai',
  resumeCommand: `cd '/home/me/ai' && claude --resume ${i}`,
})
const ROWS = Array.from({ length: 40 }, (_, i) => row(i))
const DETAIL = toCliDetail(ROWS[5]!, {
  first_prompts: ['first a', 'first b'],
  last_prompts: ['last a'],
  git_branch: 'main',
  model: 'claude-opus-5-5',
  version: '2.1.288',
  path: '/home/me/.claude/projects/-home-me-ai/x.jsonl',
})

// bodyRows 24 less 9 chrome rows (every button line fits 130 columns): a
// window of 15 sessions.
const ROOM = 15
const PANE = {
  component: 'Pane',
  requestId: 'sessile',
  viewport: { columns: 134, rows: 40, isFullscreen: false },
  props: {
    title: 'Sessions',
    isFocused: true,
    bodyColumns: 130,
    placement: 'inline',
    scroll: { offset: 0, bodyRows: 24 },
    view: {},
  },
} as const

// A fake CLI over a mutable row list: delete removes, so the cursor logic is
// what keeps a row under the ring, not a stale fixture.
function fakeCli(on: On, calls: string[][]) {
  let rows = [...ROWS]
  on('ui.focus', () => ({}))
  on('session.root', () => ({ value: '/home/me/ai' }))
  on('session.id', () => ({ value: ROWS[0]!.id }))
  on('process.run', (_, e) => {
    const argv = [...e.argv]
    calls.push(argv)
    const sub = subcommand(argv)
    const id = argv[argv.indexOf(sub!) + 1]
    if (sub === 'delete' && argv.includes('--yes')) rows = rows.filter(r => r.id !== id)
    const targets = [{ id, paths: ['/home/me/.claude/x.jsonl'] }]
    const out =
      sub === 'list'
        ? { items: rows.map(toCli), has_more: false, partial: false }
        : sub === 'get'
          ? DETAIL
          : sub === 'delete'
            ? { targets, changed: argv.includes('--yes'), requires_confirmation: argv.includes('--dry-run') }
            : {}
    const stdout = JSON.stringify(out)
    return { value: { exitCode: 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
}

test('the list, detail and confirm views draw valid trees on terminal and desktop', async ($, on) => {
  const calls: string[][] = []
  fakeCli(on, calls)
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ plugin: 'sessile', surface, ...PANE })
    await ui.press({ key: 'k:g' })
    expect(await ui.find({ key: `row:${ROWS[5]!.id}` })).toBeDefined()
    expect(await ui.find({ key: 'query' })).toBeDefined()
    // Enter on the ring row.
    await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: `row:${ROWS[5]!.id}`, origin: { kind: 'person' } })
    await ui.press({ key: `row:${ROWS[5]!.id}` })
    expect(await ui.find({ type: 'Text', text: /FIRST/ })).toBeDefined()
    await ui.press({ key: 'k:d' })
    expect(await ui.find({ type: 'Text', text: /Delete permanently/ })).toBeDefined()
    await ui.press({ key: 'no' })
    expect(await ui.find({ key: 'k:back' })).toBeDefined()
    await ui.press({ key: 'k:back' })
    expect(await ui.find({ key: 'query' })).toBeDefined()
    await ui.unmount()
  }
  expect(calls.every(argv => argv.slice(1, 6).includes('--current') && argv.includes('--json'))).toBe(true)
})

test('delete removes the row and the cursor takes the next one', async ($, on) => {
  const calls: string[][] = []
  fakeCli(on, calls)
  const ui = await $.ui.mount({ plugin: 'sessile', surface: 'terminal', ...PANE })
  await ui.press({ key: 'k:g' })
  await ui.press({ key: `row:${ROWS[5]!.id}` })
  await ui.press({ key: 'k:d' })
  await ui.press({ key: 'k:y' })
  expect(calls.some(argv => subcommand(argv) === 'delete' && argv.includes('--yes'))).toBe(true)
  expect(await ui.find({ key: `row:${ROWS[5]!.id}` })).toBeUndefined()
  expect(await ui.find({ key: `row:${ROWS[6]!.id}` })).toBeDefined()
  // The row actions are drawn only for a cursor row inside the window.
  expect(await ui.find({ key: 'k:d' })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: /Deleted “session 5”/ })).toBeDefined()
  await ui.unmount()
})

test('moving onto ▼ from the last row slides the window by one', async ($, on) => {
  fakeCli(on, [])
  const ui = await $.ui.mount({ plugin: 'sessile', surface: 'terminal', ...PANE })
  await ui.press({ key: 'k:g' })
  const last = `row:${ROWS[ROOM - 1]!.id}`
  const move = (element: string) =>
    $.ui.focus({ component: 'Pane', requestId: 'sessile', element, origin: { kind: 'person' } })
  await move(last)
  expect(await ui.find({ key: 'k:d' })).toBeDefined()
  await move('down')
  expect(await ui.find({ type: 'Text', text: /2-16 of 40/ })).toBeDefined()
  expect(await ui.find({ key: `row:${ROWS[ROOM]!.id}` })).toBeDefined()
  // The row actions follow the new cursor with no frame in between.
  expect(await ui.find({ key: 'k:d' })).toBeDefined()
  await ui.unmount()
})

test('paging, junk and archive toggles put the ring back on a drawn row', async ($, on) => {
  fakeCli(on, [])
  const ui = await $.ui.mount({ plugin: 'sessile', surface: 'terminal', ...PANE })
  await ui.press({ key: 'k:g' })
  // Row 2 is the first one with every action (rows 0 and 1 are live).
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: `row:${ROWS[2]!.id}`, origin: { kind: 'person' } })
  await ui.press({ key: 'down' })
  expect(await ui.find({ type: 'Text', text: /16-30 of 40/ })).toBeDefined()
  expect(await ui.find({ key: 'k:d' })).toBeDefined()
  await ui.press({ key: 'up' })
  expect(await ui.find({ type: 'Text', text: /1-15 of 40/ })).toBeDefined()
  expect(await ui.find({ key: 'k:i' })).toBeDefined()
  await ui.press({ key: 'k:x' })
  expect(await ui.find({ key: 'k:i' })).toBeDefined()
  await ui.press({ key: 'k:x' })
  await ui.press({ key: 'k:v' })
  expect(await ui.find({ type: 'Text', text: /Archive · 40/ })).toBeDefined()
  expect(await ui.find({ key: 'k:i' })).toBeDefined()
  await ui.unmount()
})
