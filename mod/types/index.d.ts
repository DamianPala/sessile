export type SessionRow = {
  id: string
  title: string
  titleSource: 'custom' | 'ai' | 'prompt' | 'none'
  updatedMs: number
  sizeBytes: number
  prompts: number
  isLive: boolean
  isPinned: boolean
  firstPrompt: string | null
  cwd: string | null
  // Built by the CLI, which knows the platform's shell quoting.
  resumeCommand: string
  score?: number
  matchedIn?: 'title' | 'prompt' | 'assistant'
  snippet?: string
}

export type SessionDetail = SessionRow & {
  firstPrompts: string[]
  lastPrompts: string[]
  gitBranch: string | null
  model: string | null
  version: string | null
  path: string
}

export type EmptySession = SessionRow & { paths: string[] }

// The CLI's JSON (`sessile schema <command>`), snake_case; `fromCli` turns it
// into the rows above where it arrives.
export type CliRow = {
  id: string
  title: string
  title_source: SessionRow['titleSource']
  updated_at: string
  created_at: string | null
  size_bytes: number
  prompts: number
  is_live: boolean
  is_current: boolean
  is_pinned: boolean
  first_prompt: string | null
  cwd: string | null
  resume_command: string
  interactive: boolean | null
  score?: number
  matched_in?: SessionRow['matchedIn']
  fuzzy?: boolean
  snippet?: string
}

export type CliDetail = CliRow & {
  first_prompts: string[]
  last_prompts: string[]
  git_branch: string | null
  model: string | null
  version: string | null
  path: string
}

export type CliPage = {
  items: CliRow[]
  has_more: boolean
  partial: boolean
  unreadable?: { path: string; message: string }[]
}

export type Mode =
  | { kind: 'list' }
  | { kind: 'detail'; detail: SessionDetail }
  | { kind: 'rename'; row: SessionRow; detail: SessionDetail | null }
  | { kind: 'confirm-delete'; row: SessionRow; paths: string[]; detail: SessionDetail | null }
  | { kind: 'confirm-empty'; sessions: EmptySession[] }

// The CLI call that reverses the action the notice reports, offered as `u`.
export type Undo = { args: string[]; text: string }

export type Notice = { kind: 'busy' | 'ok' | 'error'; text: string; undo?: Undo } | null

// The fetched sessions, stored apart from the rest of the view: a state write
// costs in proportion to the value, and the ring moves on every arrow key.
export type Listing = {
  rows: SessionRow[]
  searchHits: SessionRow[] | null
}

export type ViewState = Omit<View, keyof Listing>

export type View = Listing & {
  currentId: string
  query: string
  isArchiveView: boolean
  isAllProjects: boolean
  isJunkOnly: boolean
  offset: number
  cursorId: string | null
  focusKey: string | null
  // The rename field's text, kept here so a redraw never resets what was typed.
  draft: string
  mode: Mode
  notice: Notice
  isBandOpen: boolean
}

declare module 'claude-code' {
  interface PluginState {
    sessile: { view: ViewState; listing: Listing }
  }
}
