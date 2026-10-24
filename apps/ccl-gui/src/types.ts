export type Theme = 'system' | 'light' | 'dark'
export type Language = 'en' | 'zh-CN'

export interface Preferences {
  theme: Theme
  language: Language
}

export interface LibraryEntry {
  key: string
  id: string
  kind: 'vanilla' | 'managed'
  minecraft: string
  loader: string | null
  game_directory: string
}

export interface Bootstrap {
  root: string
  root_exists: boolean
  config_path: string
  preferences: Preferences
  entries: LibraryEntry[]
  warnings: string[]
  prototype: boolean
}

export interface TaskSnapshot {
  id: number
  revision: number
  state: 'running' | 'cancelling' | 'succeeded' | 'failed' | 'cancelled'
  completed: number
  total: number
  logs: string[]
  error: string | null
}

export function acceptTaskSnapshot(
  current: TaskSnapshot | null,
  incoming: TaskSnapshot | null,
): TaskSnapshot | null {
  if (!incoming) return current
  if (!current || incoming.id > current.id) return incoming
  if (incoming.id < current.id || incoming.revision <= current.revision) return current
  return incoming
}
