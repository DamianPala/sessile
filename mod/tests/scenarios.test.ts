import type { On } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { expect, test } from 'claude-code/testing'

import { cellWidth, exportFileName, listPlan } from '../hooks/view'
import { type Cli, fakeCli, OTHER, OTHER_FROM, ROOT, subcommand } from './fake-cli'

function mount($: Engine, columns: number, bodyRows: number, placement: 'inline' | 'dock' = 'inline') {
  return $.ui.mount({
    plugin: 'sessile',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'sessile',
    viewport: { columns: columns + 4, rows: bodyRows + 6, isFullscreen: placement === 'dock' },
    props: { title: 'Sessions', isFocused: true, bodyColumns: columns, placement, scroll: { offset: 0, bodyRows }, view: {} },
  })
}

const cells = (s: string) => Array.from(s).reduce((n, ch) => n + cellWidth(ch), 0)
const rowKey = (i: number) => `row:${String(i).padStart(8, '0')}-0000-4000-8000-000000000000`
const person = { kind: 'person' } as const

test('row labels fit the pane at every size, wide characters included', async ($, on) => {
  fakeCli(on, 60)
  for (const [columns, bodyRows] of [[40, 10], [80, 24], [200, 60]] as const) {
    const ui = await mount($, columns, bodyRows)
    await ui.press({ key: 'k:g' })
    const rows = (await ui.findAll({ type: 'Button' })).filter(b => b.key?.startsWith('row:'))
    expect(rows.length).toBe(listPlan(bodyRows, columns, 1).size)
    // The pin and the marker take three cells in front of every label.
    for (const row of rows) expect(cells(String(row.props.label))).toBeLessThanOrEqual(columns - 3)
    expect(new Set(rows.map(r => cells(String(r.props.label)))).size).toBe(1)
    await ui.unmount()
  }
})

test('a low inline pane keeps its rows, its paging and every key', async ($, on) => {
  const cli = fakeCli(on, 60)
  // What an 80x24 terminal gives an inline pane.
  const ui = await mount($, 80, 6)
  await ui.press({ key: 'k:g' })
  const rowKeys = async () => (await ui.findAll({ type: 'Button' })).map(b => b.key).filter(k => k?.startsWith('row:'))
  const first = await rowKeys()
  expect(first.length).toBe(2)
  for (const key of ['query', 'down', 'k:v', 'k:w', 'k:x', 'k:f', 'k:g', 'k:q']) expect(await ui.find({ key })).toBeDefined()
  expect(await ui.find({ key: 'up' })).toBeUndefined()
  await ui.press({ key: 'down' })
  const second = await rowKeys()
  expect(second.length).toBe(2)
  expect(second[0]).not.toBe(first[0])
  expect(await ui.find({ key: 'up' })).toBeDefined()
  // The ring row is one of the two drawn, and its actions are there to press.
  expect(second).toContain(`row:${cli.view.cursorId}`)
  for (const key of ['k:a', 'k:d', 'k:p', 'k:m', 'k:i']) expect(await ui.find({ key })).toBeDefined()
  await ui.unmount()
})

// The kit runs acts one at a time, so a second press issued together with the
// first reaches the handler of the dialog it was aimed at after that dialog
// closed: the stale press a doubled key makes before the redraw.
test('only the docked pane paints its background, in the configured colour', { options: { dockBackground: '#1a1b26' } }, async ($, on) => {
  fakeCli(on)
  const docked = await mount($, 80, 24, 'dock')
  expect((await docked.find({ type: 'Box' }))?.props.backgroundColor).toBe('#1a1b26')
  await docked.unmount()
  const inline = await mount($, 80, 24)
  expect((await inline.find({ type: 'Box' }))?.props.backgroundColor).toBeUndefined()
  await inline.unmount()
})

test('a doubled key on a closed dialog deletes once', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await ui.press({ key: rowKey(5) })
  await ui.press({ key: 'k:d' })
  await Promise.all([ui.press({ key: 'k:y' }), ui.press({ key: 'k:y' })])
  expect(cli.calls.filter(a => subcommand(a) === 'delete' && !a.includes('--dry-run')).length).toBe(1)
  expect(await ui.find({ type: 'Text', text: /failed/ })).toBeUndefined()
  await ui.unmount()
})

test('archiving a full-text hit keeps the search open', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  // Typing reaches onInput before Enter, as it does from a real Input.
  await $.ui.input({ plugin: 'sessile', key: 'query', text: 'session 2', kind: 'change' })
  await $.ui.input({ plugin: 'sessile', key: 'query', text: 'session 2' })
  const hits = cli.view.searchHits?.length ?? 0
  expect(hits).toBeGreaterThan(1)
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(20), origin: person })
  await ui.press({ key: 'k:a' })
  expect(cli.view.searchHits?.length).toBe(hits - 1)
  expect(await ui.find({ type: 'Text', text: /full-text “session 2”/ })).toBeDefined()
  expect(await ui.find({ key: 'k:a' })).toBeDefined()
  await ui.unmount()
})

test('a failing CLI paints the reason and the next reload recovers', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  cli.failNext = 'json'
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /Scanning sessions failed/ })).toBeDefined()
  cli.failNext = 'exit'
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /injected failure/ })).toBeDefined()
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /failed/ })).toBeUndefined()
  expect(await ui.find({ key: rowKey(2) })).toBeDefined()
  await ui.unmount()
})

test('a missing or mismatched CLI says what to do instead of "no sessions"', async ($, on) => {
  const cli = fakeCli(on, 0, 0)
  const ui = await mount($, 120, 24)
  cli.failNext = 'missing'
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /The sessile CLI was not found: “sessile”/ })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: /Install it .* cliPath/ })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: /No sessions in this project/ })).toBeUndefined()
  cli.failNext = 'usage'
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /are of different versions/ })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: /unexpected argument '--current'/ })).toBeDefined()
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /No sessions in this project/ })).toBeDefined()
  expect(await ui.find({ type: 'Text', text: /sessile CLI/ })).toBeUndefined()
  await ui.unmount()
})

test('a list missing an unreadable transcript shows the rest and says so', async ($, on) => {
  const cli = fakeCli(on, 10, 0)
  const ui = await mount($, 120, 24)
  cli.failNext = 'partial'
  await ui.press({ key: 'k:g' })
  expect(cli.view.rows.length).toBe(9)
  expect(cli.view.notice).toEqual({ kind: 'error', text: '1 transcript(s) could not be read and are left out: Permission denied' })
  await ui.press({ key: 'k:g' })
  expect(cli.view.rows.length).toBe(10)
  expect(cli.view.notice).toBeNull()
  await ui.unmount()
})

test('titles starting with a dash and pasted control characters reach the CLI as text', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(2), origin: person })
  await ui.press({ key: 'k:t' })
  await $.ui.input({ plugin: 'sessile', key: 'rename', text: '-v\u001b[2J new' })
  const call = cli.calls.find(a => subcommand(a) === 'rename')
  expect(call?.slice(-2)).toEqual(['--', '-v [2J new'])
  expect(await ui.find({ type: 'Text', text: /Renamed to/ })).toBeDefined()
  await ui.unmount()
})

test('all projects lists the other project and acts on its rows without --project', async ($, on) => {
  const cli = fakeCli(on, 10, 3)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  expect(cli.view.rows.every(r => r.cwd === ROOT)).toBe(true)
  expect(await ui.find({ key: 'k:e' })).toBeDefined()
  await ui.press({ key: 'k:w' })
  expect(cli.view.isAllProjects).toBe(true)
  expect(cli.view.rows.length).toBe(13)
  expect(await ui.find({ type: 'Text', text: /all projects/ })).toBeDefined()
  // Delete-empty works on one project, so it leaves the footer here.
  expect(await ui.find({ key: 'k:e' })).toBeUndefined()
  const other = rowKey(OTHER_FROM)
  const label = String((await ui.find({ key: other }))?.props.label)
  expect(label.startsWith('other')).toBe(true)
  // Enter on the ring row.
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: other, origin: person })
  await ui.press({ key: other })
  expect(cli.view.mode.kind).toBe('detail')
  await ui.press({ key: 'k:a' })
  const archive = cli.calls.find(a => subcommand(a) === 'archive')
  expect(archive?.includes('--project')).toBe(false)
  expect(cli.archive.map(r => r.cwd)).toEqual([OTHER])
  expect(cli.view.notice?.kind).toBe('ok')
  // Back to this project, the other project's rows are gone again.
  await ui.press({ key: 'k:w' })
  expect(cli.view.rows.every(r => r.cwd === ROOT)).toBe(true)
  await ui.unmount()
})

// /resume as Claude Code runs it: the window moves into the session, or, with
// `miss`, stays where it is and says why as text.
function stubResume(on: On, cli: Cli, miss?: string): string[] {
  const resumed: string[] = []
  on('command.run', { command: 'resume' }, (_, e) => {
    resumed.push(e.args)
    if (miss !== undefined) return { text: miss }
    cli.current = e.args
    return { text: '' }
  })
  return resumed
}

test('r says so when /resume did not move into the session', async ($, on) => {
  const cli = fakeCli(on, 10)
  const id = rowKey(5).slice(4)
  stubResume(on, cli, `Session ${id} was not found.`)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(5), origin: person })
  await ui.press({ key: 'k:r' })
  expect(cli.view.notice?.kind).toBe('error')
  expect(cli.view.notice?.text).toMatch(/^Resuming failed: Session .* was not found/)
  expect(cli.view.currentId).not.toBe(id)
  await ui.unmount()
})

test('r resumes a session of this project in place and keeps the pane on it', async ($, on) => {
  const cli = fakeCli(on, 10)
  const resumed = stubResume(on, cli)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(5), origin: person })
  await ui.press({ key: 'k:r' })
  expect(resumed).toEqual([rowKey(5).slice(4)])
  expect(cli.view.notice?.text).toMatch(/^Resumed/)
  expect(cli.view.focusKey).toBe(rowKey(5))
  // Live rows (the current one among them) and archived ones offer no resume.
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(1), origin: person })
  expect(await ui.find({ key: 'k:r' })).toBeUndefined()
  await ui.unmount()
})

test('r on another project copies its command instead, since /resume cannot reach it', async ($, on) => {
  const cli = fakeCli(on, 10, 3)
  const resumed = stubResume(on, cli)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:w' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(OTHER_FROM), origin: person })
  await ui.press({ key: 'k:r' })
  expect(resumed).toEqual([])
  expect(cli.view.notice?.text).toContain(`cd '${OTHER}' && claude --resume ${rowKey(OTHER_FROM).slice(4)}`)
  // A row of this project in the same all-projects view still resumes.
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(5), origin: person })
  await ui.press({ key: 'k:r' })
  expect(resumed).toEqual([rowKey(5).slice(4)])
  await ui.unmount()
})

test('r refuses a session that went live after the list was drawn', async ($, on) => {
  const cli = fakeCli(on, 10)
  const resumed = stubResume(on, cli)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(5), origin: person })
  cli.live = cli.live.map(r => (r.id === rowKey(5).slice(4) ? { ...r, isLive: true } : r))
  await ui.press({ key: 'k:r' })
  expect(resumed).toEqual([])
  expect(cli.view.notice?.text).toMatch(/live in another terminal/)
  await ui.unmount()
})

test('export writes into the session directory, or the configured one', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(3), origin: person })
  await ui.press({ key: 'k:m' })
  const call = cli.calls.find(a => subcommand(a) === 'export')
  // The file name is derived here: the CLI takes a destination path only.
  const name = exportFileName(cli.live[3]!.title, cli.live[3]!.id)
  expect(call).toContain(`--output-file=${ROOT}/${name}`)
  expect(call).not.toContain('--json')
  expect(cli.view.notice).toEqual({ kind: 'ok', text: `Exported ${name} to ~/ai` })
  // A live session exports too: reading it changes nothing.
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(1), origin: person })
  expect(await ui.find({ key: 'k:m' })).toBeDefined()
  await ui.unmount()
})

test('export goes where /config points', { options: { exportDir: '~/exports' } }, async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(3), origin: person })
  await ui.press({ key: 'k:m' })
  const name = exportFileName(cli.live[3]!.title, cli.live[3]!.id)
  expect(cli.calls.find(a => subcommand(a) === 'export')).toContain(`--output-file=~/exports/${name}`)
  await ui.unmount()
})

test('a pin from the list sorts the row to the top and the ring follows it', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  // Live rows pin too: a pin never touches the transcript.
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(1), origin: person })
  expect(await ui.find({ key: 'k:p' })).toBeDefined()
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(7), origin: person })
  const stars = async () => (await ui.findAll({ type: 'Text', text: '★' })).length
  const before = await stars()
  await ui.press({ key: 'k:p' })
  expect(cli.view.rows[0]?.id).toBe(rowKey(7).slice(4))
  expect(cli.view.focusKey).toBe(rowKey(7))
  expect(String((await ui.find({ key: 'k:p' }))?.props.label)).toBe('unpin')
  expect(await stars()).toBe(before + 1)
  await ui.press({ key: 'k:p' })
  expect(await stars()).toBe(before)
  expect(cli.view.rows[0]?.id).toBe(rowKey(0).slice(4))
  expect(cli.view.focusKey).toBe(rowKey(7))
  await ui.unmount()
})

test('a toggle that empties the list leaves the ring on its own button, so the same key goes back', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(3), origin: person })
  await ui.press({ key: 'k:v' })
  expect(cli.view.isArchiveView).toBe(true)
  expect(cli.view.focusKey).toBe('k:v')
  await ui.press({ key: 'k:v' })
  expect(cli.view.isArchiveView).toBe(false)
  await ui.unmount()
})

test('archiving the last row a filter shows leaves the ring on undo, not in the filter', async ($, on) => {
  // Eleven rows: every title differs, and only row 9 is "session 9".
  const cli = fakeCli(on, 11)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.input({ plugin: 'sessile', key: 'query', text: 'session 9', kind: 'change' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(9), origin: person })
  await ui.press({ key: 'k:a' })
  expect(cli.view.focusKey).toBe('k:u')
  await ui.press({ key: 'k:u' })
  expect(cli.view.rows.some(r => r.id === rowKey(9).slice(4))).toBe(true)
  await ui.unmount()
})

test('a click selects a row and a second click opens it; Enter on the ring row opens at once', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await ui.press({ key: rowKey(4) })
  expect(cli.view.mode.kind).toBe('list')
  expect(cli.view.focusKey).toBe(rowKey(4))
  await ui.press({ key: rowKey(6) })
  expect(cli.view.mode.kind).toBe('list')
  expect(cli.view.cursorId).toBe(rowKey(6).slice(4))
  await ui.press({ key: rowKey(6) })
  expect(cli.view.mode.kind).toBe('detail')
  await ui.unmount()
})

test('the click that hands the pane its keys only selects, even on the ring row', async ($, on) => {
  const cli = fakeCli(on)
  const lit = await mount($, 120, 24)
  await lit.press({ key: 'k:g' })
  await lit.press({ key: rowKey(4) })
  await lit.unmount()
  const ui = await $.ui.mount({
    plugin: 'sessile',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'sessile',
    viewport: { columns: 124, rows: 30, isFullscreen: false },
    props: { title: 'Sessions', isFocused: false, bodyColumns: 120, placement: 'inline', scroll: { offset: 0, bodyRows: 24 }, view: {} },
  })
  await ui.press({ key: rowKey(4) })
  expect(cli.view.mode.kind).toBe('list')
  expect(cli.view.focusKey).toBe(rowKey(4))
  await ui.unmount()
})

test('a pin from the detail keeps the detail open, and delete warns about it', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await ui.press({ key: rowKey(4) })
  await ui.press({ key: rowKey(4) })
  await ui.press({ key: 'k:p' })
  expect(cli.view.mode.kind).toBe('detail')
  expect(String((await ui.find({ key: 'k:p' }))?.props.label)).toBe('unpin')
  await ui.press({ key: 'k:d' })
  expect(await ui.find({ type: 'Text', text: /pinned; deleting drops the pin/ })).toBeDefined()
  await ui.unmount()
})

test('u undoes an archive and the ring goes back to the row', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(3), origin: person })
  const title = cli.view.rows.find(r => r.id === rowKey(3).slice(4))?.title
  await ui.press({ key: 'k:a' })
  expect(cli.archive.map(r => r.id)).toEqual([rowKey(3).slice(4)])
  expect(cli.view.notice?.undo?.args).toEqual(['restore', rowKey(3).slice(4)])
  await ui.press({ key: 'k:u' })
  expect(cli.archive).toEqual([])
  expect(cli.view.notice).toEqual({ kind: 'ok', text: `Restored “${title}”` })
  expect(cli.view.focusKey).toBe(rowKey(3))
  // The undo is spent: the notice no longer offers it.
  expect(await ui.find({ key: 'k:u' })).toBeUndefined()
  await ui.unmount()
})

test('switching to the archive keeps a full-text search and runs it there', async ($, on) => {
  const cli = fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  await $.ui.input({ plugin: 'sessile', key: 'query', text: 'session', kind: 'change' })
  await $.ui.input({ plugin: 'sessile', key: 'query', text: 'session' })
  expect(cli.view.searchHits).not.toBeNull()
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: rowKey(2), origin: person })
  await ui.press({ key: 'k:v' })
  expect(cli.view.isArchiveView).toBe(true)
  expect(cli.view.searchHits).toEqual([])
  const last = cli.calls.filter(a => subcommand(a) === 'search').at(-1)
  expect(last).toContain('--archived')
  const reads = cli.calls.filter(a => subcommand(a) === 'search' || subcommand(a) === 'list')
  expect(reads.every(a => a.includes('--include-headless'))).toBe(true)
  expect(await ui.find({ type: 'Text', text: /full-text “session”/ })).toBeDefined()
  await ui.unmount()
})

test('without the keys the caps are drawn dim and the hint says where the keys are', async ($, on) => {
  fakeCli(on)
  const ui = await $.ui.mount({
    plugin: 'sessile',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'sessile',
    viewport: { columns: 124, rows: 30, isFullscreen: true },
    props: { title: 'Sessions', isFocused: false, bodyColumns: 120, placement: 'dock', scroll: { offset: 0, bodyRows: 24 }, view: {} },
  })
  await ui.press({ key: 'k:g' })
  // A string `text` matches a substring; the cap is the whole text.
  expect((await ui.find({ type: 'Text', text: /^i$/ }))?.props.dimColor).toBe(true)
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: 'query', origin: person })
  expect(await ui.find({ type: 'Text', text: /keys are with the prompt/ })).toBeDefined()
  await ui.unmount()
})

test('with the ring in the filter the footer caps are dim and the hint teaches the rows', async ($, on) => {
  fakeCli(on)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  expect((await ui.find({ type: 'Text', text: /^v$/ }))?.props.dimColor).toBe(false)
  await $.ui.focus({ component: 'Pane', requestId: 'sessile', element: 'query', origin: person })
  expect((await ui.find({ type: 'Text', text: /^v$/ }))?.props.dimColor).toBe(true)
  expect(await ui.find({ type: 'Text', text: /↓ rows, then the letters act/ })).toBeDefined()
  await ui.unmount()
})

test('deleting the last row moves the cursor up, deleting the only row goes to the filter', async ($, on) => {
  const cli = fakeCli(on, 4)
  const ui = await mount($, 120, 24)
  await ui.press({ key: 'k:g' })
  for (const i of [3, 2]) {
    await ui.press({ key: rowKey(i) })
    await ui.press({ key: 'k:d' })
    await ui.press({ key: 'k:y' })
    expect(cli.view.focusKey).toBe(rowKey(i - 1))
  }
  cli.live = cli.live.filter(r => !r.isLive)
  await ui.press({ key: 'k:g' })
  expect(await ui.find({ type: 'Text', text: /No sessions in this project/ })).toBeDefined()
  await ui.unmount()
})
