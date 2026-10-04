import { atom, read, update } from 'claude-code'
import type { Elements, EngineInterface, Register, UiPressArgument } from 'claude-code'

import type { CliDetail, CliPage, CliRow, EmptySession, Listing, Mode, Notice, SessionDetail, SessionRow, Undo, View } from '../types'
import {
  back,
  bytes,
  cells,
  clampOffset,
  clean,
  cleanDetail,
  cleanRow,
  columnsFor,
  detailFromCli,
  edgeStep,
  emptyListing,
  emptyView,
  exportFileName,
  followCursor,
  fromCli,
  FOOTER_KEYS,
  hasFilter,
  headerLabel,
  highlight,
  isArmed,
  isDrawnInList,
  isOnRows,
  joinPath,
  keyWidth,
  shortLabel,
  listPlan,
  LIVE_NOTE,
  GLYPHS,
  type Glyph,
  marker,
  packLines,
  projectSlug,
  ROW_ACTION_KEYS,
  ROW_KEYS,
  rowLabel,
  settleTarget,
  shortPath,
  stripControl,
  visibleRows,
  windowOffset,
  withoutListing,
} from './view'

const PANE = 'sessile'
const view = atom({ plugin: 'sessile', key: 'view' } as const, withoutListing(emptyView))
const listing = atom({ plugin: 'sessile', key: 'listing' } as const, emptyListing)

type Patch = Partial<View> | ((v: View) => Partial<View>)
type Els = Pick<Elements['terminal'], 'Box' | 'Text' | 'Button' | 'Input'>
// `isFocused`: whether the site holds the keys; a docked pane keeps drawing
// after the person went back to the prompt.
type Room = { columns: number; bodyRows: number; isFocused: boolean; onClose: () => void }
type Surface = UiPressArgument['surface']
type SiteEvent = { component: string; requestId: string; plugin?: string }

// Set by register on every (re)load; the userConfig values are only reachable there.
let cliPath = 'sessile'
let layout: 'pane' | 'band' = 'pane'
let dockBackground = ''
let exportDir = ''
// The ui.focus event carries no size; the last drawn site's is the one the person sees.
let lastBodyRows = 30
let lastColumns = 80
let regrownAt = 0
// The band's id is the engine's; it is known once the band has drawn.
let bandId: string | null = null
let isActing = false
// Where takeKeysBack is putting the ring; null when it is not running.
let reclaimKey: string | null = null
// Whether the site held the keys at its last drawing; the band takes them
// only through ctrl+x tab, so it counts as holding them.
let paneHasKeys = true

// A read copies the whole value out of the engine, every row of it, and the
// ring moves on every arrow key. Only patch writes the listing, so the module
// keeps what it last wrote or read; after a reload the first read fills it.
let listingCache: Listing | null = null

async function readListing($: EngineInterface): Promise<Listing> {
  listingCache ??= await read($, listing)
  return listingCache
}

async function readView($: EngineInterface): Promise<View> {
  const { rows, searchHits } = await readListing($)
  return { ...withoutListing(await read($, view)), rows, searchHits }
}

// The listing is written only when a change brings new rows, so a move of the
// ring writes a few hundred bytes, not every session. Every listing change
// also writes the view, and that write is what redraws: the drawing reads the
// listing from the cache, which is set before it.
async function patch($: EngineInterface, change: Patch) {
  const list = await readListing($)
  const fresh: { listing?: Listing } = {}
  await update($, view, state => {
    const { rows, searchHits, ...rest } = typeof change === 'function' ? change({ ...withoutListing(state), ...list }) : change
    if ((rows !== undefined && rows !== list.rows) || (searchHits !== undefined && searchHits !== list.searchHits)) {
      fresh.listing = { rows: rows ?? list.rows, searchHits: searchHits === undefined ? list.searchHits : searchHits }
      listingCache = fresh.listing
    }
    return withoutListing({ ...state, ...rest })
  })
  if (fresh.listing !== undefined) await update($, listing, () => fresh.listing ?? list)
}

function siteId(): string {
  return layout === 'band' ? bandId ?? PANE : PANE
}

function isOurSite(e: SiteEvent): boolean {
  if (e.component === 'Pane') return e.requestId === PANE
  return layout === 'band' && e.requestId === bandId && (e.plugin === undefined || e.plugin === 'sessile')
}

type CliCall = { text?: string; isAll?: boolean; isJson?: boolean }

// Global flags go first and free text after `--`, so a title or query
// starting with "-" is never read as a flag. Without --project an id is looked
// up in every project, which is where a row of the all-projects list lives;
// `isAll` defaults to the scope the view shows. `isJson: false` is for a call
// that writes its result to a file: stdout stays empty, and a failure still
// comes as a JSON object, since stderr here is not a terminal.
async function cli($: EngineInterface, args: string[], call: CliCall = {}): Promise<unknown> {
  const all = call.isAll ?? (await readView($)).isAllProjects
  const scope = all ? [] : ['--project', await $.session.root()]
  const current = await $.session.id()
  const free = call.text === undefined ? [] : ['--', call.text]
  const format = call.isJson === false ? [] : ['--json']
  const argv = [cliPath, ...scope, '--current', current, ...format, ...args, ...free]
  // A binary that cannot be started is reported by the engine, as a thrown
  // error or as the run's stderr; both read "failed to start".
  const run = await $.process.run(argv, { timeoutMs: 60_000 }).catch((error: unknown) => {
    const stderr = error instanceof Error ? error.message : String(error)
    return { exitCode: -1, stdout: '', stderr, isStdoutTruncated: false }
  })
  if (run.exitCode !== 0 && run.stderr.includes('failed to start')) {
    setupProblem = [`The sessile CLI was not found: “${cliPath}”`, 'Install it (see the plugin README), or set its path: /config → sessile → cliPath']
    throw new Error('the sessile CLI was not found')
  }
  if (run.isStdoutTruncated) throw new Error('sessile output is over the 4 MiB read limit')
  const reason = parseReason(run.stderr) ?? `sessile exited ${run.exitCode}`
  // The pane never sends a call the CLI of its own version rejects, so a
  // usage error means the two are of different versions.
  if (run.exitCode === 2) {
    setupProblem = ['The sessile CLI and this pane are of different versions', `Update both to the same release · the CLI said: ${reason}`]
    throw new Error('the sessile CLI is of another version than the pane')
  }
  setupProblem = null
  if (run.exitCode !== 0) {
    const page = partialPage(run.stdout)
    if (page === null) throw new Error(reason)
    partialNote = reason
    return page
  }
  return run.stdout.trim() === '' ? {} : JSON.parse(run.stdout)
}

// A list or search that could not read every transcript still prints what it
// read, marked partial, and exits 1: the rows are shown and the action's
// notice says what is missing, unless the action has its own to give.
let partialNote: string | null = null

// Why no call can work, with what to do about it: the binary is missing or of
// another version. Drawn in place of the list, since an empty list would read
// as "no sessions"; any call that gets through clears it.
let setupProblem: [string, string] | null = null

function partialPage(stdout: string): CliPage | null {
  try {
    const page = JSON.parse(stdout) as CliPage
    return page.partial === true && Array.isArray(page.items) ? page : null
  } catch {
    return null
  }
}

// One wrapper for every action: busy notice, errors drawn red, never a thrown
// hook. While one runs every other is ignored, so a doubled key cannot delete
// twice or let a slow reload overwrite a view opened after it. Resolves to
// whether the work ran and succeeded: only then did the view change, so only
// then may the caller move the ring into it.
async function act($: EngineInterface, label: string, work: () => Promise<Partial<View> | void>): Promise<boolean> {
  // Checked and set in one synchronous step: a read-then-write on the state
  // lets two presses in the same tick both through.
  if (isActing) return false
  isActing = true
  try {
    await patch($, { notice: { kind: 'busy', text: `${label}…` } })
    partialNote = null
    const result = (await work()) ?? {}
    const notice: Notice = partialNote === null ? null : { kind: 'error', text: clean(partialNote) }
    await patch($, { notice, ...result })
    return true
  } catch (error) {
    const message = clean(error instanceof Error ? error.message : String(error))
    await patch($, { notice: { kind: 'error', text: `${label} failed: ${message}` } })
    return false
  } finally {
    isActing = false
    await keepRingOffUndo($)
  }
}

// The undo button goes with the notice that offered it, and any action
// replaces that notice; a ring left on the button would rest on nothing.
async function keepRingOffUndo($: EngineInterface) {
  const v = await readView($)
  if (v.focusKey !== 'k:u' || v.notice?.undo !== undefined) return
  if (v.mode.kind === 'list') return focusRow($, v.cursorId)
  // A dialog draws the notice too: the ring goes back to its first element.
  await focusKey($, v.mode.kind === 'rename' ? 'rename' : v.mode.kind === 'detail' ? 'k:back' : 'no')
}

// A press can reach a handler drawn before the last change (a doubled key
// before the redraw): it acts only while its row is still listed and, for a
// dialog, while that dialog is still open.
async function isStale($: EngineInterface, row: SessionRow, dialog?: Mode['kind']): Promise<boolean> {
  const v = await readView($)
  if (dialog !== undefined && v.mode.kind !== dialog) return true
  return ![...v.rows, ...(v.searchHits ?? [])].some(r => r.id === row.id)
}

function done(text: string): Notice {
  return { kind: 'ok', text }
}

// When the element holding the ring leaves the tree (a deleted row, a closed
// dialog) the ring lands on nothing and the arrows stop, so every view change
// puts it somewhere on purpose. The key is recorded up front so the drawing
// matches the ring from the next frame; callers only name elements the view
// they just set draws. A deny means the pane does not hold the keys, and the
// person's next move there records the real key.
async function focusKey($: EngineInterface, key: string) {
  await patch($, { focusKey: key })
  await $.ui.focus({ requestId: siteId(), key }).catch(() => undefined)
}

async function focusRow($: EngineInterface, id: string | null) {
  const v = await readView($)
  const rows = visibleRows(v)
  const index = id === null ? -1 : rows.findIndex(r => r.id === id)
  if (index < 0) return focusKey($, 'query')
  const room = listRoom(lastBodyRows, lastColumns, v)
  const drawn = windowOffset(v, rows, room)
  const isInView = index >= drawn && index < drawn + room
  const offset = isInView ? drawn : clampOffset(index - Math.floor(room / 2), rows.length, room)
  await patch($, { offset, cursorId: id })
  return focusKey($, `row:${id}`)
}

// Reloads and toggles redraw the list under the ring. A ring on the rows goes
// to the cursor row if still listed, else the window's first row; one on a
// footer button that went away goes to the nearest one. With no row left it
// goes to `pressed`, the button that emptied the list: in the filter the
// letters would type, so the same key could not undo the toggle.
async function settle($: EngineInterface, pressed = 'query') {
  const v = await readView($)
  if (v.mode.kind !== 'list') return
  const key = v.focusKey
  const room = listRoom(lastBodyRows, lastColumns, v)
  if (key !== null && !isOnRows(key)) {
    if (!isDrawnInList(v, key, room)) await focusKey($, key === 'k:e' ? 'k:v' : 'query')
    return
  }
  const rows = visibleRows(v)
  const target = settleTarget(v.cursorId, rows, windowOffset(v, rows, room), room)
  if (target === null && isDrawnInList(v, pressed, room)) return focusKey($, pressed)
  await focusRow($, target)
}

// Paging and scrolling. With the ring on the rows the cursor moves with the
// window in the same write, so the drawing never snaps back to the old row.
async function moveWindow($: EngineInterface, by: (room: number) => number) {
  const v = await readView($)
  if (v.mode.kind !== 'list') return
  const rows = visibleRows(v)
  const room = listRoom(lastBodyRows, lastColumns, v)
  const offset = clampOffset(windowOffset(v, rows, room) + by(room), rows.length, room)
  const id = isOnRows(v.focusKey) ? settleTarget(v.cursorId, rows, offset, room) : null
  if (id === null) return patch($, { offset })
  await patch($, { offset, cursorId: id, focusKey: `row:${id}` })
  await focusKey($, `row:${id}`)
}

async function backTo($: EngineInterface, mode: Mode) {
  const target = back(mode)
  await patch($, { mode: target })
  if (target.kind === 'detail') return focusKey($, 'k:back')
  const id = 'row' in mode ? mode.row.id : mode.kind === 'detail' ? mode.detail.id : (await readView($)).cursorId
  await focusRow($, id)
}

function archivedArgs(isArchiveView: boolean): string[] {
  return isArchiveView ? ['--archived'] : []
}

// Which sessions the list shows: live or archived, this project or all.
type Scope = Pick<View, 'isArchiveView' | 'isAllProjects'>

function scopeOf(v: View): Scope {
  return { isArchiveView: v.isArchiveView, isAllProjects: v.isAllProjects }
}

// The pane shows headless sessions too: clearing out what `claude -p` and the
// Agent SDK leave behind is one of its jobs.
function scopeArgs(scope: Scope): string[] {
  return [...archivedArgs(scope.isArchiveView), ...(scope.isAllProjects ? ['--all'] : []), '--include-headless']
}

// The engine reads at most 4 MiB from a process (about 500 bytes a row), so a
// list keeps the newest this many; the CLI's own default is far smaller.
const LIST_LIMIT = 5000
// A full-text search returns the best hits only; a full page says "first".
const SEARCH_LIMIT = 50

async function listRows($: EngineInterface, scope: Scope) {
  const args = ['list', ...scopeArgs(scope), `--limit=${LIST_LIMIT}`]
  const page = (await cli($, args, { isAll: scope.isAllProjects })) as CliPage
  return page.items.map(r => cleanRow(fromCli(r)))
}

async function searchRows($: EngineInterface, query: string, scope: Scope) {
  const page = (await cli($, ['search', ...scopeArgs(scope), `--limit=${SEARCH_LIMIT}`], { text: query, isAll: scope.isAllProjects })) as CliPage
  return page.items.map(r => cleanRow(fromCli(r)))
}

// The rows after a change, with an open full-text search run again so the
// list the person was in stays the list they see.
async function freshRows($: EngineInterface): Promise<Partial<View>> {
  const v = await readView($)
  const rows = await listRows($, scopeOf(v))
  if (v.searchHits === null) return { rows }
  return { rows, searchHits: await searchRows($, v.query, scopeOf(v)) }
}

// `choose` gets the scope as it is when the work starts, so two quick toggles
// land where the second one points. An open full-text search runs again in
// the new scope, as it does after an action (freshRows).
async function reload($: EngineInterface, choose: (scope: Scope) => Scope, pressed?: string) {
  const label = (await readView($)).searchHits === null ? 'Scanning sessions' : 'Searching'
  const ran = await act($, label, async () => {
    const v = await readView($)
    const scope = choose(scopeOf(v))
    return {
      rows: await listRows($, scope),
      currentId: await $.session.id(),
      ...scope,
      searchHits: v.searchHits === null ? null : await searchRows($, v.query, scope),
      offset: 0,
      mode: { kind: 'list' },
    }
  })
  if (ran) await settle($, pressed)
}

function search($: EngineInterface, query: string) {
  return act($, 'Searching', async () => {
    if (query.trim() === '') return { searchHits: null, offset: 0 }
    const hits = await searchRows($, query, scopeOf(await readView($)))
    // Typing on while the search ran cleared the hits; these answer older text.
    if ((await readView($)).query !== query) return {}
    return { searchHits: hits, offset: 0 }
  })
}

async function fetchDetail($: EngineInterface, id: string) {
  const { isArchiveView } = await readView($)
  return cleanDetail(detailFromCli((await cli($, ['get', id, ...archivedArgs(isArchiveView)])) as CliDetail))
}

// A click raises no ui.focus before its press, so the ring still shows where
// the person was: on this row the press is Enter or a second click, and opens
// the row; anywhere else it is a first click, which only selects it, as does
// the click that hands the pane the keys.
async function pressRow($: EngineInterface, row: SessionRow) {
  const v = await readView($)
  if (paneHasKeys && v.focusKey === `row:${row.id}`) return openRow($, row)
  if (await isStale($, row, 'list')) return
  if (paneHasKeys) return focusRow($, row.id)
  await patch($, { cursorId: row.id, focusKey: `row:${row.id}` })
  await takeKeysBack($)
}

async function openRow($: EngineInterface, row: SessionRow) {
  if (await isStale($, row, 'list')) return
  const ran = await act($, 'Opening', async () => ({ mode: { kind: 'detail', detail: await fetchDetail($, row.id) } }))
  if (ran) await focusKey($, 'k:back')
}

// `/resume <id>` switches this process to a session of its own project only;
// for another one it says "not found" and resolves all the same, so such a
// session gets the command for a terminal of its own instead. A session that
// went live since the list was drawn is refused: two processes would append
// to one transcript.
async function resumeRow($: EngineInterface, row: SessionRow, surface: Surface) {
  if (await isStale($, row)) return
  const v = await readView($)
  const root = await $.session.root()
  const isHere = !v.isAllProjects || (row.cwd !== null && projectSlug(row.cwd) === projectSlug(root))
  if (!isHere) return showResumeCommand($, row.resumeCommand, surface)
  const ran = await act($, 'Resuming', async () => {
    if ((await fetchDetail($, row.id)).isLive) throw new Error('it is live in another terminal')
    const answer = await $.command.run({ command: 'resume', args: row.id })
    // /resume reports a miss as text, not as an error: the session this
    // window is in afterwards tells whether it switched.
    if ((await $.session.id()) !== row.id) throw new Error(clean(answer.text ?? '') || 'Claude Code stayed in this session')
    return {
      ...(await freshRows($)),
      currentId: await $.session.id(),
      mode: { kind: 'list' },
      notice: done(`Resumed “${row.title || row.id.slice(0, 8)}”`),
    }
  })
  if (ran) await focusRow($, row.id)
}

// The command is drawn even where the clipboard is out of reach (a narrow
// pane clips its tail, so the copy is what a person normally uses).
async function showResumeCommand($: EngineInterface, command: string, surface: Surface) {
  const r = await $.ui.copy({ text: command, surface }).catch((error: unknown) => ({ isCopied: false, reason: String(error) }))
  const text = `${r.isCopied ? 'Copied for a new terminal' : 'Run in a new terminal'}: ${command}`
  await patch($, { notice: { kind: 'ok', text } })
}

async function startRename($: EngineInterface, row: SessionRow, detail: SessionDetail | null) {
  if (await isStale($, row, detail === null ? 'list' : 'detail')) return
  await patch($, { mode: { kind: 'rename', row, detail }, draft: row.title })
  await focusKey($, 'rename')
}

async function rename($: EngineInterface, row: SessionRow, title: string, from: Mode) {
  if (await isStale($, row, 'rename')) return
  const next = clean(title)
  if (next === '' || next === row.title) return backTo($, from)
  const ran = await act($, 'Renaming', async () => {
    await cli($, ['rename', row.id], { text: next })
    return { ...(await freshRows($)), mode: { kind: 'list' }, notice: done(`Renamed to “${next}”`) }
  })
  if (ran) await focusRow($, row.id)
}

async function askDelete($: EngineInterface, row: SessionRow, detail: SessionDetail | null) {
  if (await isStale($, row, detail === null ? 'list' : 'detail')) return
  const ran = await act($, 'Checking', async () => {
    const { isArchiveView } = await readView($)
    const plan = (await cli($, ['delete', row.id, '--dry-run', ...archivedArgs(isArchiveView)])) as { targets: { paths: string[] }[] }
    return { mode: { kind: 'confirm-delete', row, paths: plan.targets[0]?.paths ?? [], detail } }
  })
  if (ran) await focusKey($, 'no')
}

async function askEmpty($: EngineInterface) {
  const ran = await act($, 'Checking', async () => {
    const plan = (await cli($, ['delete-empty', '--dry-run'], { isAll: false })) as { targets: (CliRow & { paths: string[] })[] }
    if (plan.targets.length === 0) return { notice: done('No empty sessions.') }
    const sessions: EmptySession[] = plan.targets.map(t => ({ ...cleanRow(fromCli(t)), paths: t.paths }))
    return { mode: { kind: 'confirm-empty', sessions } }
  })
  if (ran && (await readView($)).mode.kind === 'confirm-empty') await focusKey($, 'no')
}

// After the list reloads the ring goes to the same session if it is still
// listed, else to the row that took its place.
async function runAndReload($: EngineInterface, label: string, args: string[], message: string, subjectId: string | null, undo?: Undo) {
  const index = visibleRows(await readView($)).findIndex(r => r.id === subjectId)
  const ran = await act($, label, async () => {
    await cli($, args)
    const notice: Notice = undo === undefined ? done(message) : { kind: 'ok', text: message, undo }
    return { ...(await freshRows($)), mode: { kind: 'list' }, notice }
  })
  if (!ran) return
  const rows = visibleRows(await readView($))
  const target = rows.find(r => r.id === subjectId) ?? rows[Math.min(Math.max(index, 0), rows.length - 1)]
  // With no row left the ring would go to the filter, which types the `u`.
  if (target === undefined && undo !== undefined) return focusKey($, 'k:u')
  await focusRow($, target?.id ?? null)
}

// No dialog, since both are reversible; `u` reverses one instead, for an `a`
// pressed by habit or held down.
async function archiveOrRestore($: EngineInterface, row: SessionRow, isArchiveView: boolean) {
  if (await isStale($, row)) return
  const name = row.title || row.id
  return isArchiveView
    ? runAndReload($, 'Restoring', ['restore', row.id], `Restored “${name}”`, row.id, { args: ['archive', row.id], text: `Archived “${name}” again` })
    : runAndReload($, 'Archiving', ['archive', row.id], `Archived “${name}”`, row.id, { args: ['restore', row.id], text: `Restored “${name}”` })
}

// Read from the state at press time: a press drawn before the last action
// must not undo a newer one.
async function undoLast($: EngineInterface) {
  const undo = (await readView($)).notice?.undo
  if (undo === undefined) return
  return runAndReload($, 'Undoing', undo.args, undo.text, undo.args[1] ?? null)
}

async function deleteRow($: EngineInterface, row: SessionRow) {
  if (await isStale($, row, 'confirm-delete')) return
  // The dialog was the confirmation; --yes passes it on.
  const args = ['delete', row.id, '--yes', ...archivedArgs((await readView($)).isArchiveView)]
  return runAndReload($, 'Deleting', args, `Deleted “${row.title || row.id}”`, row.id)
}

async function deleteEmpty($: EngineInterface, count: number) {
  const v = await readView($)
  if (v.mode.kind !== 'confirm-empty') return
  return runAndReload($, 'Deleting', ['delete-empty', '--yes'], `Deleted ${count} empty sessions`, v.cursorId)
}

// From the list the ring follows the row to where its pin sorts it; from the
// detail the detail stays open, redrawn with the new state.
async function togglePin($: EngineInterface, row: SessionRow, detail: SessionDetail | null) {
  if (await isStale($, row, detail === null ? undefined : 'detail')) return
  const args = [row.isPinned ? 'unpin' : 'pin', row.id]
  const label = row.isPinned ? 'Unpinning' : 'Pinning'
  const message = `${row.isPinned ? 'Unpinned' : 'Pinned'} “${row.title || row.id}”`
  if (detail === null) return runAndReload($, label, args, message, row.id)
  await act($, label, async () => {
    await cli($, args)
    return { ...(await freshRows($)), mode: { kind: 'detail', detail: await fetchDetail($, row.id) }, notice: done(message) }
  })
}

// Into the directory of the session the pane runs in unless /config names
// another, so an old session can be handed to this one as a file.
async function exportRow($: EngineInterface, row: SessionRow) {
  if (await isStale($, row)) return
  await act($, 'Exporting', async () => {
    const dir = exportDir === '' ? await $.session.root() : exportDir
    const path = joinPath(dir, exportFileName(row.title, row.id))
    const args = ['export', row.id, `--output-file=${path}`, ...archivedArgs((await readView($)).isArchiveView)]
    await cli($, args, { isJson: false })
    // The file name first: a notice is cut at its end, and the name is what
    // tells this export from the others in the directory.
    return { notice: done(`Exported ${exportFileName(row.title, row.id)} to ${shortPath(clean(dir))}`) }
  })
}

async function copyText($: EngineInterface, text: string, what: string, surface: Surface) {
  try {
    const r = await $.ui.copy({ text, surface })
    await patch($, { notice: r.isCopied ? done(`Copied ${what}`) : { kind: 'error', text: `Copy failed: ${r.reason}` } })
  } catch (error) {
    await patch($, { notice: { kind: 'error', text: `Copy failed: ${error instanceof Error ? error.message : String(error)}` } })
  }
}

async function toggleJunk($: EngineInterface) {
  await patch($, s => ({ isJunkOnly: !s.isJunkOnly, offset: 0 }))
  await settle($, 'k:x')
}

async function clearSearch($: EngineInterface) {
  await patch($, { searchHits: null, query: '', offset: 0 })
  await settle($)
}

async function openPane($: EngineInterface) {
  // A dialog in a pane still open stays as it was; a pane opened anew starts
  // at the list, not at the detail it was closed from.
  const isOpen = layout === 'band' ? (await readView($)).isBandOpen : (await $.ui.panes()).some(p => p.id === PANE)
  if (!isOpen) await patch($, { mode: { kind: 'list' } })
  const isList = (await readView($)).mode.kind === 'list'
  // Every opening starts at the live sessions of this project.
  const home: Scope = { isArchiveView: false, isAllProjects: false }
  if (layout === 'band') {
    await patch($, { isBandOpen: true })
    if (isList) void reload($, () => home)
    return { text: 'sessile opened above the prompt: ctrl+x tab gives it the keys.' }
  }
  await openPaneSite($)
  if (isList) void reload($, () => home)
  return { text: 'sessile pane opened.' }
}

// Oversized on purpose: the engine caps both at what the layout spares.
function openPaneSite($: EngineInterface) {
  return $.ui.open({ id: PANE, title: 'sessile', focus: true, closeOnEscape: true, holdToasts: true, rows: 500, columns: 500 })
}

// Escape hands the keys to the prompt before ui.close runs, so a close the
// pane refuses leaves it drawn without them. Opening it again asks for them
// back (granted over an empty composer) and puts the ring on the filter; it
// then goes where the step back put it. The opening's move to the filter is
// recorded by a ui.focus hook that can finish after this one, so it is not
// recorded at all, and the key is written once the ring is there.
async function takeKeysBack($: EngineInterface) {
  const key = (await readView($)).focusKey
  reclaimKey = key
  try {
    await openPaneSite($).catch(() => undefined)
    if (key === null) return
    const moved = await $.ui.focus({ requestId: PANE, key }).catch(() => ({ deny: 'thrown' }))
    if (moved.deny === undefined) await patch($, { focusKey: key })
  } finally {
    reclaimKey = null
  }
}

export const register: Register = (on, options) => {
  cliPath = String(options.cliPath ?? 'sessile')
  layout = options.layout === 'band' ? 'band' : 'pane'
  dockBackground = String(options.dockBackground ?? '').trim()
  exportDir = String(options.exportDir ?? '').trim()

  on('session.start', async ($, e, next) => {
    const description = 'Browse, search, resume, rename, archive, export and delete sessions, here or in every project'
    await $.command.register({ name: 'sessile', description })
    // Other mods register /sessions too; the alias may lose, /sessile stays.
    await $.command.register({ name: 'sessions', description: `${description} (alias of /sessile)` }).catch(() => undefined)
    return next(e)
  })

  on('command.run', { command: 'sessile' }, openPane)
  on('command.run', { command: 'sessions' }, openPane)

  // Esc steps back one level: a filled filter clears first, as in fzf; only
  // Esc on the plain list closes the pane.
  on('ui.close', async ($, e, next) => {
    if (e.id !== PANE || e.origin.kind !== 'person') return next(e)
    const v = await readView($)
    if (v.mode.kind === 'list' && !hasFilter(v)) return next(e)
    await (v.mode.kind === 'list' ? clearSearch($) : backTo($, v.mode))
    void takeKeysBack($)
    return { value: undefined }
  })

  on('ui.scroll', async ($, e, next) => {
    if (!isOurSite(e) || (await readView($)).mode.kind !== 'list') return next(e)
    await moveWindow($, () => e.by)
    return {}
  })

  on('ui.focus', async ($, e, next) => {
    if (!isOurSite(e)) return next(e)
    const element = e.element ?? null
    // Checked before any await: the reclaim may be over by the time one returns.
    if (reclaimKey !== null && e.origin.kind === 'plugin' && element !== reclaimKey) return next(e)
    const v = await readView($)
    if (v.mode.kind !== 'list') {
      await patch($, { focusKey: element })
      return next(e)
    }
    const rows = visibleRows(v)
    const room = listRoom(lastBodyRows, lastColumns, v)
    const drawn = windowOffset(v, rows, room)
    const step = e.origin.kind === 'person' ? edgeStep(v.focusKey, element, rows, drawn, room) : null
    if (step !== null) {
      // The row may only come into view with the next drawing, which
      // $.ui.focus waits for; the ring stays put until then.
      const key = `row:${step.id}`
      await patch($, { offset: step.offset, cursorId: step.id, focusKey: key })
      void focusKey($, key)
      return {}
    }
    if (element?.startsWith('row:') === true) {
      const id = element.slice(4)
      const offset = followCursor(drawn, rows.findIndex(r => r.id === id), rows.length, room)
      // A success notice is about the row it was on; an offered undo stays
      // until the next action.
      const isNoticeDone = e.origin.kind === 'person' && element !== v.focusKey && v.notice?.kind === 'ok' && v.notice.undo === undefined
      await patch($, { cursorId: id, focusKey: element, offset, ...(isNoticeDone ? { notice: null } : {}) })
    } else {
      await patch($, { focusKey: element })
    }
    return next(e)
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const v = await readView($)
    if (layout !== 'band' || !v.isBandOpen || e.props.hasSurvey || e.surface === 'mobile') return next(e)
    bandId = e.requestId
    paneHasKeys = true
    lastBodyRows = e.props.maxRows - 1
    lastColumns = Math.max(40, e.props.bodyColumns - 6)
    return draw($, $.ui.resolve(e), v, {
      columns: lastColumns,
      bodyRows: lastBodyRows,
      // The band takes the keys only through ctrl+x tab; its caps stay lit.
      isFocused: true,
      onClose: () => void patch($, { isBandOpen: false }),
    })
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    if (e.surface === 'mobile') {
      const { Text } = $.ui.resolve(e)
      return <Text>Sessions are managed from a terminal or desktop session.</Text>
    }
    const v = await readView($)
    // An inline block takes its content's height, and the tree takes the
    // block's: from 0 neither grows. Half the screen breaks the circle; the
    // engine answers with the rows it really gives. A block that shrank with
    // the terminal stays that low after the terminal grows back, for the same
    // reason: one far below what this screen gives is asked for again, once
    // per screen height.
    const given = e.props.scroll?.bodyRows ?? 0
    const screenRows = e.viewport?.rows ?? 30
    const isLeftLow = e.props.placement !== 'dock' && given > 0 && given < Math.floor(screenRows / 6) && regrownAt !== screenRows
    if (isLeftLow) regrownAt = screenRows
    lastBodyRows = given > 0 && !isLeftLow ? given : Math.floor(screenRows / 2)
    lastColumns = Math.max(40, e.props.bodyColumns ?? e.viewport?.columns ?? 80)
    paneHasKeys = e.props.isFocused !== false
    const els = $.ui.resolve(e)
    const tree = draw($, els, v, {
      columns: lastColumns,
      bodyRows: lastBodyRows,
      isFocused: paneHasKeys,
      onClose: () => void $.ui.close({ id: PANE }),
    })
    // The dock carries Claude Code's own shade. No palette index is the
    // terminal's background, so matching it takes the colour itself.
    if (e.props.placement !== 'dock' || dockBackground === '') return tree
    const { Box } = els
    return <Box flexDirection="column" width="100%" minHeight={lastBodyRows} backgroundColor={dockBackground}>{tree}</Box>
  })
}

function planFor(bodyRows: number, columns: number, v: View) {
  return listPlan(bodyRows, columns, v.searchHits !== null ? 2 : 1)
}

function listRoom(bodyRows: number, columns: number, v: View): number {
  return planFor(bodyRows, columns, v).size
}

// `isArmed: false` draws the cap dim: its key does not reach the pane now.
// `form`: the cap and a bracketed button, or in the compact list a plain
// button the engine draws as `k: label`, with the label shortened if asked.
type KeyForm = 'caps' | 'plain' | 'short'
type KeyStyle = { isDanger?: boolean; isQuiet?: boolean; isArmed?: boolean; form?: KeyForm }
type Item = { node: JSX.Element; width: number }

// The key cap sits beside the Button: a plain Button would show the hotkey but
// not the brackets, a bracketed one hides it. `cap` is drawn in place of the
// hotkey for a button the keys reach some other way (esc).
function keyBtn(els: Els, key: string, label: string, onPress: (press: UiPressArgument) => unknown, style: KeyStyle = {}, cap?: string): Item {
  const { Box, Text, Button } = els
  const isArmed = style.isArmed ?? true
  const color = isArmed ? (style.isDanger === true ? 'red' : 'cyan') : undefined
  const hotkey = cap === undefined ? key : undefined
  if (style.form === 'plain' || style.form === 'short') {
    const text = style.form === 'short' ? shortLabel(label) : label
    const plain = <Button key={`k:${key}`} plain label={text} hotkey={key} dimColor={!isArmed || style.isQuiet} onPress={press => void onPress(press)} />
    return { node: plain, width: cells(key) + 2 + cells(text) }
  }
  const node = (
    <Box flexDirection="row" gap={1} flexShrink={0}>
      <Text bold={isArmed} dimColor={!isArmed} color={color}>{cap ?? key}</Text>
      <Button key={`k:${key}`} label={label} hotkey={hotkey} dimColor={style.isQuiet} onPress={press => void onPress(press)} />
    </Box>
  )
  return { node, width: keyWidth(cap ?? key, label) }
}

function textItem(els: Els, text: string, color: string): Item {
  const { Text } = els
  return { node: <Text color={color}>{text}</Text>, width: cells(text) }
}

// Items packed into as many lines as the width needs, each clipped, never
// wrapped: listChrome plans the same packing for the widest labels.
function buttonLines(els: Els, columns: number, items: Item[]) {
  const { Box } = els
  const lines = packLines(items.map(item => item.width), columns).map(line => (
    <Box flexDirection="row" gap={2} height={1} overflow="hidden">{line.map(i => items[i]?.node)}</Box>
  ))
  return <Box flexDirection="column">{lines}</Box>
}

function noticeLine($: EngineInterface, els: Els, notice: Notice, isFocused: boolean) {
  const { Box, Text } = els
  if (notice === null) return null
  const color = notice.kind === 'error' ? 'red' : notice.kind === 'ok' ? 'green' : undefined
  const text = <Text color={color} dimColor={notice.kind === 'busy'} wrap="truncate-end">{notice.text}</Text>
  if (notice.undo === undefined) return text
  return (
    <Box flexDirection="row" gap={2} height={1} overflow="hidden">
      {text}
      {keyBtn(els, 'u', 'undo', () => undoLast($), { isArmed: isFocused }).node}
    </Box>
  )
}

function draw($: EngineInterface, els: Els, v: View, room: Room) {
  const mode = v.mode
  if (mode.kind === 'detail') return drawDetail($, els, v, mode.detail, room)
  if (mode.kind === 'rename') return drawRename($, els, v, mode, room)
  if (mode.kind === 'confirm-delete') return drawConfirmDelete($, els, v, mode, room)
  if (mode.kind === 'confirm-empty') return drawConfirmEmpty($, els, v, mode.sessions, room)
  return drawList($, els, v, room)
}

type ListWindow = { rows: SessionRow[]; shown: SessionRow[]; offset: number; below: number }

function drawList($: EngineInterface, els: Els, v: View, room: Room) {
  const { Box } = els
  const rows = visibleRows(v)
  const { size, isCompact } = planFor(room.bodyRows, room.columns, v)
  const offset = windowOffset(v, rows, size)
  const shown = rows.slice(offset, offset + size)
  const win: ListWindow = { rows, shown, offset, below: rows.length - offset - shown.length }
  const key = v.focusKey ?? ''
  const isRowFocus = key.startsWith('row:') || ROW_ACTION_KEYS.includes(key)
  const cursor = isRowFocus ? shown.find(r => r.id === v.cursorId) ?? null : null
  // A tree taller than the body turns the arrows into pane scrolling instead of
  // moving the cursor, so the list is pinned to the body height and every
  // chrome line to one row (clipped, never wrapped, in a narrow docked pane).
  if (isCompact) {
    const top = !room.isFocused ? legendLine(els, false) : v.notice !== null ? noticeLine($, els, v.notice, true) : listHeader(els, v, win)
    // Whole labels while the line holds them, the short ones otherwise.
    const fitted = (lead: number, items: (form: KeyForm) => Item[]) => {
      const plain = items('plain')
      const width = plain.reduce((sum, item) => sum + item.width + 2, lead)
      return (width - 2 <= room.columns ? plain : items('short')).map(item => item.node)
    }
    const paging = win.below > 0 ? cells(`n: ▼ ${win.below}`) + 2 : 0
    return (
      <Box flexDirection="column" height={room.bodyRows} overflow="hidden">
        <Box flexDirection="row" height={1} overflow="hidden">{top}</Box>
        {listBody($, els, v, win, room, true)}
        <Box flexDirection="row" gap={2} height={1} overflow="hidden">
          {win.below > 0 && downButton($, els, win, true)}
          {cursor !== null ? fitted(paging, form => rowItems($, els, v, cursor, null, room.isFocused, form)) : hintLine(els, v, room)}
        </Box>
        <Box flexDirection="row" gap={2} height={1} overflow="hidden">{fitted(0, form => footerItems($, els, v, room, form))}</Box>
      </Box>
    )
  }
  return (
    <Box flexDirection="column" height={room.bodyRows} overflow="hidden">
      {listHeader(els, v, win)}
      {listBody($, els, v, win, room, false)}
      {cursor !== null ? rowActions($, els, v, cursor, room) : hintLine(els, v, room)}
      {legendLine(els, room.isFocused)}
      {listFooter($, els, v, room)}
      {noticeLine($, els, v.notice, room.isFocused)}
    </Box>
  )
}

// In the row actions' place while the ring is off the rows: what the keys do
// from here.
function hintLine(els: Els, v: View, room: Room) {
  const { Box, Text } = els
  const key = v.focusKey
  const hint = !room.isFocused
    ? 'Letters typed now go to Claude, not to the list'
    : key === null || key === 'query'
      ? 'Type to filter · Enter: search text · ↓ rows, then the letters act'
      : `↑ rows · Enter presses · Esc ${hasFilter(v) ? 'clears the filter' : 'closes'}`
  return <Box height={1} overflow="hidden"><Text dimColor wrap="truncate-end">{hint}</Text></Box>
}

function glyphText(els: Els, g: Glyph) {
  const { Text } = els
  return <Text color={g.color} dimColor={g.dim}>{g.glyph}</Text>
}

// A key the pane does not bind falls through to the prompt and takes the
// keyboard with it, and a mod cannot hold it back. The legend's line, drawn in
// every state of the list, then says where the keys went and how to get them.
const KEYS_LEFT = 'The keys are with the prompt · click the pane or run /sessile to use them'

function legendLine(els: Els, isFocused: boolean) {
  const { Box, Text } = els
  if (!isFocused) return <Box height={1} overflow="hidden"><Text color="yellow" wrap="truncate-end">{KEYS_LEFT}</Text></Box>
  return (
    <Box flexDirection="row" gap={2} height={1} overflow="hidden">
      {Object.values(GLYPHS).map(g => <Text dimColor>{glyphText(els, g)} {g.label}</Text>)}
    </Box>
  )
}

function listHeader(els: Els, v: View, win: ListWindow) {
  const { Box, Text } = els
  const position = win.rows.length === 0 ? '' : `${win.offset + 1}-${win.offset + win.shown.length} of ${win.rows.length}`
  return (
    <Box flexDirection="row" justifyContent="space-between" height={1} overflow="hidden">
      <Box flexDirection="row" gap={1}>
        <Text bold>{v.isArchiveView ? 'Archive' : 'Sessions'}{v.isAllProjects ? ' · all projects' : ''} · {v.rows.length >= LIST_LIMIT ? `newest ${v.rows.length}` : v.rows.length}</Text>
        {v.isJunkOnly && <Text color="yellow" inverse> junk </Text>}
        {v.searchHits !== null && <Text color="cyan" inverse> full-text “{clean(v.query)}” · {v.searchHits.length >= SEARCH_LIMIT ? `best ${SEARCH_LIMIT}` : v.searchHits.length} hits </Text>}
        <Text dimColor>{position}</Text>
      </Box>
    </Box>
  )
}

// The paging buttons stay next to the rows in the tree in both layouts: the
// arrows reach them from the first and the last row, which scrolls the list.
function upButton($: EngineInterface, els: Els, win: ListWindow, isShort: boolean) {
  const { Button } = els
  return <Button key="up" plain hotkey="b" label={`▲ ${win.offset}${isShort ? '' : ' more'}`} onPress={() => void moveWindow($, room => -room)} />
}

function downButton($: EngineInterface, els: Els, win: ListWindow, isShort: boolean) {
  const { Button } = els
  return <Button key="down" plain hotkey="n" label={`▼ ${win.below}${isShort ? '' : ' more'}`} onPress={() => void moveWindow($, room => room)} />
}

function listBody($: EngineInterface, els: Els, v: View, win: ListWindow, room: Room, isCompact: boolean) {
  const { Box, Text, Button, Input } = els
  const cols = columnsFor(room.columns, v.isAllProjects)
  const now = Date.now()
  const filter = <Input key="query" label="› " placeholder="filter · Enter: full-text search · ↓: rows · f: back here" value={v.query} submitLabel="search" autoFocus onInput={query => patch($, { query: stripControl(query), offset: 0, searchHits: null })} onSubmit={query => search($, query)} />
  return (
    <Box flexDirection="column">
      {isCompact
        ? <Box flexDirection="row" gap={2} height={1} overflow="hidden"><Box flexGrow={1}>{filter}</Box>{win.offset > 0 && upButton($, els, win, true)}</Box>
        : filter}
      {!isCompact && <Text dimColor wrap="truncate-end">   {headerLabel(cols)}</Text>}
      {!isCompact && (win.offset > 0 ? upButton($, els, win, false) : <Text> </Text>)}
      {win.shown.length === 0 && setupProblem === null && <Text dimColor wrap="truncate-end">{emptyReason(v)}</Text>}
      {win.shown.length === 0 && setupProblem !== null && <Text color="red" wrap="wrap">{setupProblem[0]}</Text>}
      {win.shown.length === 0 && setupProblem !== null && <Text wrap="wrap">{setupProblem[1]}</Text>}
      {win.shown.map(row => {
        const m = marker(row, v.currentId)
        return (
          <Box flexDirection="column">
            <Box flexDirection="row" height={1} overflow="hidden">
              {row.isPinned ? glyphText(els, GLYPHS.pinned) : <Text> </Text>}
              {glyphText(els, m)}
              <Text> </Text>
              <Button key={`row:${row.id}`} plain label={rowLabel(row, now, cols)} onPress={() => void pressRow($, row)} />
            </Box>
            {row.snippet !== undefined && snippetLine(els, row, v.query)}
          </Box>
        )
      })}
      {!isCompact && (win.below > 0 ? downButton($, els, win, false) : <Text> </Text>)}
    </Box>
  )
}

// Scope and filters first, so a pane too narrow even for two lines loses the
// least needed keys.
function listFooter($: EngineInterface, els: Els, v: View, room: Room) {
  return buttonLines(els, room.columns, footerItems($, els, v, room))
}

function footerItems($: EngineInterface, els: Els, v: View, room: Room, form: KeyForm = 'caps'): Item[] {
  const style = { isQuiet: true, isArmed: isArmed(v, room.isFocused), form }
  const F = FOOTER_KEYS
  const items = [
    keyBtn(els, 'v', F.v[v.isArchiveView ? 1 : 0], () => reload($, s => ({ ...s, isArchiveView: !s.isArchiveView }), 'k:v'), style),
    keyBtn(els, 'w', F.w[v.isAllProjects ? 1 : 0], () => reload($, s => ({ ...s, isAllProjects: !s.isAllProjects }), 'k:w'), style),
    keyBtn(els, 'x', F.x[v.isJunkOnly ? 1 : 0], () => toggleJunk($), style),
    ...(!v.isArchiveView && !v.isAllProjects ? [keyBtn(els, 'e', F.e[0], () => askEmpty($), { ...style, isDanger: true })] : []),
    ...(hasFilter(v) ? [keyBtn(els, 'c', F.c[0], () => clearSearch($), style)] : []),
    keyBtn(els, 'f', F.f[0], () => focusKey($, 'query'), style),
    keyBtn(els, 'g', F.g[0], () => reload($, s => s, 'k:g'), style),
    keyBtn(els, 'q', F.q[0], room.onClose, style),
  ]
  return items
}

// The row's actions, shared by the list and the detail.
function rowItems($: EngineInterface, els: Els, v: View, row: SessionRow, detail: SessionDetail | null, isFocused: boolean, form: KeyForm = 'caps'): Item[] {
  const style = { isArmed: isFocused, form }
  const R = ROW_KEYS
  const pin = keyBtn(els, 'p', R.p[row.isPinned ? 1 : 0], () => togglePin($, row, detail), style)
  const safe = [
    keyBtn(els, 'm', R.m[0], () => exportRow($, row), style),
    keyBtn(els, 'i', R.i[0], p => copyText($, row.id, 'session id', p.surface), style),
  ]
  if (row.isLive) return [textItem(els, LIVE_NOTE, 'yellow'), pin, ...safe]
  return [
    ...(v.isArchiveView
      ? []
      : [
          keyBtn(els, 'r', R.r[0], p => resumeRow($, row, p.surface), style),
          keyBtn(els, 't', R.t[0], () => startRename($, row, detail), style),
        ]),
    keyBtn(els, 'a', R.a[v.isArchiveView ? 1 : 0], () => archiveOrRestore($, row, v.isArchiveView), style),
    keyBtn(els, 'd', R.d[0], () => askDelete($, row, detail), { ...style, isDanger: true }),
    pin,
    ...safe,
  ]
}

function rowActions($: EngineInterface, els: Els, v: View, row: SessionRow, room: Room) {
  return buttonLines(els, room.columns, rowItems($, els, v, row, null, room.isFocused))
}

function snippetLine(els: Els, row: SessionRow, query: string) {
  const { Text } = els
  const [before, hit, after] = highlight(row.snippet ?? '', query)
  return (
    <Text dimColor wrap="truncate-end">
      {'    '}<Text italic>in {row.matchedIn ?? 'text'}</Text>{'  '}{before}<Text bold color="cyan">{hit}</Text>{after}
    </Text>
  )
}

function emptyReason(v: View): string {
  const query = clean(v.query)
  if (v.notice?.kind === 'busy') return v.notice.text
  if (v.searchHits !== null) return `Nothing in prompts or replies matches “${query}” · c clears`
  if (query !== '') return `No title matches “${query}” · Enter searches prompts and replies`
  if (v.isJunkOnly) return 'No junk by the current rule (≤1 prompt, <20K, or untitled)'
  if (v.isArchiveView) return 'Archive is empty · a on a session archives it'
  return v.isAllProjects ? 'No sessions in any project' : 'No sessions in this project'
}

// Pinned to the body height like the list, with fewer prompts in a low pane,
// so the arrows walk the buttons instead of scrolling the pane.
function drawDetail($: EngineInterface, els: Els, v: View, d: SessionDetail, room: Room) {
  const { Box, Text } = els
  const m = marker(d, v.currentId)
  const isWide = room.columns >= 90
  const keep = room.bodyRows < 16 ? 2 : 3
  const style = { isArmed: room.isFocused }
  const prompts = (title: string, list: string[]) => (
    <Box flexDirection="column" flexGrow={1} width={isWide ? '50%' : '100%'}>
      <Text dimColor bold>{title}</Text>
      {list.length === 0 ? <Text dimColor>none</Text> : list.map(p => <Text wrap="truncate-end">› {p.replace(/\s+/g, ' ')}</Text>)}
    </Box>
  )
  const updated = new Date(d.updatedMs)
  // The yellow line below says it for a live one.
  const actions = rowItems($, els, v, d, d, room.isFocused).slice(d.isLive ? 1 : 0)
  // Escape is the key for back; no letter, since `b` pages up in the list. The
  // button stays for the mouse, for the band (whose Escape the mod never
  // hears) and for a pane that could not take the keys back over a filled composer.
  const back = keyBtn(els, 'back', 'back', () => backTo($, { kind: 'detail', detail: d }), { ...style, isQuiet: true }, 'esc')
  const close = keyBtn(els, 'q', 'close', room.onClose, { ...style, isQuiet: true })
  return (
    <Box flexDirection="column" height={room.bodyRows} overflow="hidden">
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold>{d.isPinned && glyphText(els, GLYPHS.pinned)}{glyphText(els, m)} {d.title || '(untitled)'}</Text>
        {d.isLive && <Text color={m.color}>{m.glyph} {d.id === v.currentId ? 'this session' : 'live'}</Text>}
      </Box>
      <Text wrap="truncate-end">
        {d.id}   <Text dimColor>branch</Text> {d.gitBranch ?? '—'}   <Text dimColor>model</Text> {d.model ?? '—'}   {d.prompts} <Text dimColor>prompts</Text>   {bytes(d.sizeBytes)}
      </Text>
      <Text wrap="truncate-end">
        <Text dimColor>updated</Text> {updated.toISOString().slice(0, 16).replace('T', ' ')}   <Text dimColor>in</Text> {d.cwd === null ? '—' : shortPath(clean(d.cwd))}
      </Text>
      <Text> </Text>
      <Box flexDirection={isWide ? 'row' : 'column'} gap={2}>
        {prompts('FIRST', d.firstPrompts.slice(0, keep))}
        {prompts('LAST', d.lastPrompts.slice(-keep))}
      </Box>
      <Text> </Text>
      {d.isLive && <Text color="yellow">Live session: resume, rename, archive and delete are off.</Text>}
      {buttonLines(els, room.columns, [...actions, back, close])}
      {noticeLine($, els, v.notice, room.isFocused)}
    </Box>
  )
}

function drawRename($: EngineInterface, els: Els, v: View, mode: Extract<Mode, { kind: 'rename' }>, room: Room) {
  const { Box, Text, Input } = els
  const row = mode.row
  return (
    <Box flexDirection="column">
      <Text bold>Rename session</Text>
      <Text dimColor wrap="truncate-end">{rowLabel(row, Date.now(), columnsFor(room.columns))}</Text>
      <Input key="rename" label="› New title: " value={v.draft} submitLabel="rename" autoFocus onInput={draft => patch($, { draft: stripControl(draft) })} onSubmit={title => rename($, row, title, mode)} />
      <Text dimColor>Enter renames · Esc goes back</Text>
      {noticeLine($, els, v.notice, room.isFocused)}
    </Box>
  )
}

function drawConfirmDelete($: EngineInterface, els: Els, v: View, mode: Extract<Mode, { kind: 'confirm-delete' }>, room: Room) {
  const { Box, Text, Button } = els
  const row = mode.row
  // The question, the session, the count and the buttons always fit: a low
  // pane lists fewer paths and drops the blank lines.
  const fixed = 6 + (row.isPinned ? 1 : 0) + (v.notice !== null ? 1 : 0)
  const spare = room.bodyRows - fixed
  const isRoomy = spare >= mode.paths.length + 2
  const shown = isRoomy ? mode.paths : mode.paths.slice(0, Math.max(0, spare))
  return (
    <Box flexDirection="column" borderStyle="round" borderColor="red" paddingX={1}>
      <Text bold color="red">Delete permanently?</Text>
      <Text wrap="truncate-end">{row.title || row.id}   <Text dimColor>{row.prompts} prompts · {bytes(row.sizeBytes)}</Text></Text>
      {isRoomy && <Text> </Text>}
      {shown.map(p => <Text dimColor wrap="truncate-middle">{shortPath(p)}</Text>)}
      <Text dimColor wrap="truncate-end">{mode.paths.length} paths{shown.length > 0 && shown.length < mode.paths.length ? `, ${shown.length} listed` : ''} · cannot be undone</Text>
      {row.isPinned && <Text color="yellow" wrap="truncate-end">This session is pinned; deleting drops the pin too.</Text>}
      {isRoomy && <Text> </Text>}
      <Box flexDirection="row" gap={3}>
        <Box flexDirection="row" gap={1}>
          <Text bold color="cyan">n</Text>
          <Button key="no" label="cancel" hotkey="n" autoFocus onPress={() => void backTo($, mode)} />
        </Box>
        {keyBtn(els, 'y', 'delete', () => deleteRow($, row), { isDanger: true, isArmed: room.isFocused }).node}
      </Box>
      {noticeLine($, els, v.notice, room.isFocused)}
    </Box>
  )
}

function drawConfirmEmpty($: EngineInterface, els: Els, v: View, sessions: EmptySession[], room: Room) {
  const { Box, Text, Button } = els
  const cols = columnsFor(room.columns - 4)
  // A low pane keeps the question and the buttons: no column header, fewer
  // sessions, and the count of the rest on the last line.
  const isLow = room.bodyRows < 12
  const limit = Math.max(1, room.bodyRows - (isLow ? 5 : 9) - (v.notice !== null ? 1 : 0))
  const rest = sessions.length - Math.min(limit, sessions.length)
  const now = Date.now()
  return (
    <Box flexDirection="column" borderStyle="round" borderColor="red" paddingX={1}>
      <Text bold color="red" wrap="truncate-end">Delete {sessions.length} empty sessions permanently?</Text>
      {!isLow && <Text dimColor wrap="truncate-end">  {headerLabel(cols)}</Text>}
      {sessions.slice(0, limit).map(s => <Text dimColor wrap="truncate-end">  {rowLabel(s, now, cols)}</Text>)}
      {!isLow && rest > 0 && <Text dimColor>  …and {rest} more</Text>}
      <Text dimColor wrap="truncate-end">{isLow && rest > 0 ? `…and ${rest} more · ` : 'Sessions with '}no prompt and no reply · cannot be undone</Text>
      <Box flexDirection="row" gap={3}>
        <Box flexDirection="row" gap={1}>
          <Text bold color="cyan">n</Text>
          <Button key="no" label="cancel" hotkey="n" autoFocus onPress={() => void backTo($, { kind: 'confirm-empty', sessions })} />
        </Box>
        {keyBtn(els, 'y', `delete all ${sessions.length}`, () => deleteEmpty($, sessions.length), { isDanger: true, isArmed: room.isFocused }).node}
      </Box>
      {noticeLine($, els, v.notice, room.isFocused)}
    </Box>
  )
}

// The error object is the last non-empty stderr line: `{"error":{"kind","message",…}}`.
// The CLI's reason and its hint, with every session id cut to the eight
// characters the pane shows: a full id pushes the rest off a one-line notice.
function parseReason(stderr: string): string | undefined {
  const lines = stderr.trim().split('\n')
  try {
    const parsed = JSON.parse(lines[lines.length - 1] ?? '') as { error?: { message?: string; hint?: string } }
    const message = parsed.error?.message
    if (message === undefined) return undefined
    const hint = parsed.error?.hint
    return (hint === undefined ? message : `${message} · ${hint}`).replace(/\b([0-9a-f]{8})-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, '$1')
  } catch {
    return lines[0] || undefined
  }
}
