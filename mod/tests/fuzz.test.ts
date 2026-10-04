import type { Engine } from 'claude-code/testing'
import { expect, test } from 'claude-code/testing'

import type { View } from '../types'
import { listPlan, visibleRows } from '../hooks/view'
import { fakeCli, type Cli } from './fake-cli'

// Seeded random walks over everything a person can do in the pane, checking
// after every step the invariants the hand-found bugs broke. A failure prints
// the seed and the last steps, which replay the same way.
// Kept small for every run; a hunt raises both (2 x 100 seeds x 300 steps ran clean).
const SEEDS = Array.from({ length: 8 }, (_, i) => i + 1)
const STEPS = 200
const SIZES = [
  { columns: 40, bodyRows: 10 },
  { columns: 80, bodyRows: 24 },
  { columns: 200, bodyRows: 60 },
]
const QUERIES = ['', 'se', 'session 1', '日本', '🚀', '-starts', 'zzz', 'x', 'pasted\u001b[2J\ttext']

function rng(seed: number) {
  let s = seed >>> 0
  return () => {
    s = (s + 0x6d2b79f5) >>> 0
    let t = s
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function mount($: Engine, size: (typeof SIZES)[number]) {
  return $.ui.mount({
    plugin: 'sessile',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'sessile',
    viewport: { columns: size.columns + 4, rows: size.bodyRows + 6, isFullscreen: false },
    props: {
      title: 'Sessions',
      isFocused: true,
      bodyColumns: size.columns,
      placement: 'inline',
      scroll: { offset: 0, bodyRows: size.bodyRows },
      view: {},
    },
  })
}

type Mounted = Awaited<ReturnType<typeof mount>>

async function check(v: View, ui: Mounted, size: (typeof SIZES)[number], expectError: boolean) {
  if (v.notice?.kind === 'busy') throw new Error(`busy notice left behind: ${v.notice.text}`)
  if (v.notice?.kind === 'error' && !expectError) throw new Error(`unexpected error notice: ${v.notice.text}`)
  if (v.focusKey !== null && (await ui.find({ key: v.focusKey })) === undefined) {
    throw new Error(`ring on ${v.focusKey}, which is not drawn (mode ${v.mode.kind})`)
  }
  if (v.mode.kind !== 'list') return
  const drawnRows = (await ui.findAll({ type: 'Button' })).filter(b => b.key?.startsWith('row:'))
  const room = listPlan(size.bodyRows, size.columns, v.searchHits !== null ? 2 : 1).size
  if (drawnRows.length > room) throw new Error(`${drawnRows.length} rows drawn in a window of ${room}`)
  const rows = visibleRows(v)
  if (drawnRows.length < Math.min(room, rows.length)) {
    throw new Error(`only ${drawnRows.length} rows drawn for ${rows.length} rows and a window of ${room}`)
  }
  const hasActions = (await ui.find({ key: 'k:i' })) !== undefined
  if (hasActions && !drawnRows.some(b => b.key === `row:${v.cursorId}`)) {
    throw new Error(`row actions drawn for ${v.cursorId}, which is not in the window`)
  }
}

type Step = { name: string; run: () => Promise<unknown>; injects?: boolean }

async function pickStep($: Engine, ui: Mounted, cli: Cli, next: () => number, bodyRows: number): Promise<Step> {
  const pick = <T,>(list: T[]): T | undefined => list[Math.floor(next() * list.length)]
  const buttons = (await ui.findAll({ type: 'Button' })).map(b => b.key).filter((k): k is string => k !== undefined)
  const inputs = (await ui.findAll({ type: 'Input' })).map(b => b.key).filter((k): k is string => k !== undefined)
  const focusables = [...buttons, ...inputs]
  const roll = next()
  const person = { kind: 'person' } as const
  const focus = (element: string) => () =>
    $.ui.focus({ component: 'Pane', requestId: 'sessile', element, origin: person })
  if (roll < 0.4) {
    const key = pick(buttons.filter(k => k !== 'k:q'))
    if (key !== undefined) return { name: `press ${key}`, run: () => ui.press({ key }) }
  }
  if (roll < 0.7) {
    const key = pick(focusables)
    if (key !== undefined) return { name: `move to ${key}`, run: focus(key) }
  }
  if (roll < 0.8) {
    const scroll = { component: 'Pane', requestId: 'sessile', offset: 0, bodyRows, contentRows: bodyRows, origin: person } as const
    const by = pick([-5, -1, 1, 5]) ?? 1
    return { name: `scroll ${by}`, run: () => $.ui.scroll({ ...scroll, by }) }
  }
  // Typing needs the ring on the field first, as it does for a person.
  const type = (key: string, text: string, kind: 'change' | 'submit') => async () => {
    await focus(key)()
    // A real Input reports every edit before Enter.
    await $.ui.input({ plugin: 'sessile', key, text, kind: 'change' })
    if (kind === 'submit') await $.ui.input({ plugin: 'sessile', key, text, kind })
  }
  if (roll < 0.88 && inputs.includes('query')) {
    const text = pick(QUERIES) ?? ''
    const kind = next() < 0.7 ? 'change' : 'submit'
    return { name: `${kind} query "${text}"`, run: type('query', text, kind) }
  }
  if (roll < 0.95 && inputs.includes('rename')) {
    const text = pick(QUERIES) ?? 'renamed'
    return { name: `rename to "${text}"`, run: type('rename', text, 'submit') }
  }
  const fail = next() < 0.5 ? 'exit' : 'json'
  return { name: `next CLI call fails (${fail})`, injects: true, run: async () => (cli.failNext = fail) }
}

for (const seed of SEEDS) {
  test(`random walk, seed ${seed}`, async ($, on) => {
    const cli = fakeCli(on, 60)
    on('command.run', { command: 'resume' }, (_, e) => {
      cli.current = e.args
      return { text: '' }
    })
    const next = rng(seed)
    let size = SIZES[seed % SIZES.length]!
    let ui = await mount($, size)
    await ui.press({ key: 'k:g' })
    const log: string[] = []
    let isFailing = false
    for (let i = 0; i < STEPS; i++) {
      if (next() < 0.04) {
        size = SIZES[Math.floor(next() * SIZES.length)]!
        await ui.unmount()
        ui = await mount($, size)
        log.push(`resize ${size.columns}x${size.bodyRows}`)
      }
      const step = await pickStep($, ui, cli, next, size.bodyRows)
      log.push(step.name)
      const pending = cli.failNext !== null
      await step.run()
      // A failure stays possible until the injected answer was used and shown.
      isFailing = step.injects === true || pending || (isFailing && cli.view.notice?.kind === 'error')
      try {
        await check(cli.view, ui, size, isFailing)
      } catch (error) {
        throw new Error(`seed ${seed}, step ${i}: ${(error as Error).message}\n  ${log.slice(-15).join('\n  ')}`)
      }
    }
    expect(cli.calls.length).toBeGreaterThan(0)
    await ui.unmount()
  })
}
