export type MemoryPolicy = "auto" | number
export type Theme = 'system' | 'light' | 'dark'
export type Language = 'en' | 'zh-CN'

export interface Preferences { theme: Theme; language: Language; reduce_transparency: boolean; reduce_motion: boolean; last_selected_instance: string | null; background_image: string | null }
export interface LaunchDefaults { java: string | null; max_memory_mb: number; auto_memory: boolean; auto_install_java: boolean; network_offline: boolean }
export interface LibraryEntry { key: string; id: string; display_name: string; has_revision_storage: boolean; resource_root: string; profile_directory: string; delete_paths: string[]; retained_paths: string[]; dependent_profiles: string[]; minecraft: string; loader: string | null; game_directory: string; memory_override: MemoryPolicy | null; launch_version: string }
export interface Bootstrap {
  root: string
  root_exists: boolean
  setup_required: boolean
  config_path: string
  launch_defaults: LaunchDefaults
  preferences: Preferences
  entries: LibraryEntry[]
  warnings: string[]
  hidden_entries: number
}
export interface GameVersion { id: string; kind: string }
export interface LoaderVersion { version: string; stable: boolean }

export interface Authorization { user_code: string; verification_uri: string; expires_in: number }
export interface TaskSnapshot {
  id: number
  revision: number
  kind: string
  cancellable?: boolean
  phase: string
  state: 'running' | 'cancelling' | 'succeeded' | 'failed' | 'cancelled'
  completed: number
  total: number | null
  logs: string[]
  error: string | null
  error_code: string | null
  pid: number | null
  result: unknown | null
  authorization: Authorization | null
}

export interface TaskSnapshotRecord {
  id: number
  revision: number
  state: TaskSnapshot['state']
}
export interface Account {
  id: string
  username: string
  uuid: string
  account_type: string
}
export interface AccountListing { accounts: Account[]; selected: string | null }
export interface AccountStorage { available: boolean }
export interface RunRecord {
  run_id: string
  version: string
  pid: number | null
  exit_code: number | null
  cancelled: boolean
  log_path: string
  started_at_ms: number
  finished_at_ms: number | null
}
export interface LogText { text: string }
export type UpdateChannel = 'stable' | 'dev'
export interface GuiUpdateManifest {
  version: string; commit: string; channel: UpdateChannel; target: string; os_signed: boolean
  asset: { format: string; name: string; size: number; sha256: string; url: string }
}
export interface GuiSelfUpdateResponse {
  selected_channel: UpdateChannel
  build: { version: string; commit: string; channel: UpdateChannel; target: string }
  supported: boolean; reason: string | null
  status: { phase: string; version: string; channel: UpdateChannel; has_backup: boolean; last_error: string | null } | null
  check: { available: boolean; build_pending: boolean; branch_commit: string; candidate: GuiUpdateManifest | null } | null
  helper_pid: number | null; restart_required: boolean
}

export const GUI_LOG_MAX_LINES = 5_000
export const GUI_LOG_MAX_BYTES = 4 * 1024 * 1024
export const GUI_LOG_MAX_LINE_BYTES = 8 * 1024

/** Keep log text bounded before it enters React state or the DOM. */
export function boundLogLines(lines: readonly string[], maxLines = GUI_LOG_MAX_LINES, maxBytes = GUI_LOG_MAX_BYTES): string[] {
  const encoder = new TextEncoder()
  const decoder = new TextDecoder()
  const bounded: string[] = []
  let bytes = 0
  const candidates = lines.slice(-Math.max(0, maxLines))
  for (let index = candidates.length - 1; index >= 0; index--) {
    const raw = candidates[index]!
    let line = raw
    let encoded = encoder.encode(line)
    if (encoded.byteLength > GUI_LOG_MAX_LINE_BYTES) {
      encoded = encoded.slice(encoded.byteLength - (GUI_LOG_MAX_LINE_BYTES - 3))
      line = `…${decoder.decode(encoded)}`
      encoded = encoder.encode(line)
      while (encoded.byteLength > GUI_LOG_MAX_LINE_BYTES) {
        line = `…${Array.from(line.slice(1)).slice(1).join('')}`
        encoded = encoder.encode(line)
      }
    }
    const separatorBytes = bounded.length ? 1 : 0
    if (bytes + separatorBytes + encoded.byteLength > maxBytes) break
    bounded.push(line)
    bytes += separatorBytes + encoded.byteLength
  }
  return bounded.reverse()
}

export function boundLogText(text: string, maxLines = GUI_LOG_MAX_LINES, maxBytes = GUI_LOG_MAX_BYTES): string {
  return boundLogLines(text.split(/\r?\n/), maxLines, maxBytes).join('\n')
}
export interface ImportPreview { path: string; summary: unknown }

export interface InstanceRequest { id: string; minecraft: string; loader: string | null; loader_version: string | null }
export interface ImportRequest { update_existing: boolean; id: string; archive: string; include_optional: boolean; replace_modified: boolean; allowed_hosts: string[] }
export interface LaunchIdentity { kind: 'offline' | 'microsoft'; username: string | null; account_id: string | null }
export interface LaunchRequest {
  key: string
  identity: LaunchIdentity
  java: string | null
  memory: MemoryPolicy | null
  auto_install_java: boolean
  dry_run: boolean
}

export function acceptTaskSnapshot(current: TaskSnapshot | null, incoming: TaskSnapshot | null): TaskSnapshot | null {
  if (!incoming) return current
  if (!current || incoming.id > current.id) return incoming
  if (incoming.id < current.id || incoming.revision <= current.revision) return current
  return incoming
}

export function taskProgress(snapshot: Pick<TaskSnapshot, 'completed' | 'total'>): number | null {
  if (snapshot.total === null || !Number.isFinite(snapshot.total) || snapshot.total <= 0) return null
  if (!Number.isFinite(snapshot.completed)) return 0
  return Math.max(0, Math.min(100, snapshot.completed / snapshot.total * 100))
}

export function newestHistory(current: TaskSnapshot[], incoming: TaskSnapshot[]): TaskSnapshot[] {
  const byId = new Map<number, TaskSnapshot>()
  for (const item of [...current, ...incoming]) {
    const previous = byId.get(item.id)
    if (!previous || item.revision > previous.revision) byId.set(item.id, { ...item, logs: item.logs.slice(-100) })
  }
  return [...byId.values()].sort((a, b) => b.id - a.id).slice(0, 8)
}

export function retainBusyEntries(previous: Bootstrap, incoming: Bootstrap): Bootstrap {
  const listUnavailable = incoming.warnings.some(warning => /revision.*(?:busy|unreadable|locked)/i.test(warning))
  if (!listUnavailable || previous.root !== incoming.root) return incoming
  const present = new Set(incoming.entries.map(entry => entry.key))
  const retained = previous.entries.filter(entry => entry.has_revision_storage && !present.has(entry.key))
  return retained.length ? { ...incoming, entries: [...incoming.entries, ...retained] } : incoming
}

const SECRET_KEY = /(?:access.?token|refresh.?token|id.?token|client.?secret|password|credential|authorization|cookie)/i
export function redactSensitive(value: unknown): unknown {
  if (Array.isArray(value)) {
    let redactNextArgument = false
    return value.map(item => {
      if (redactNextArgument) { redactNextArgument = false; return '[redacted]' }
      if (typeof item === 'string') {
        const flag = /^--?(?:access.?token|refresh.?token|id.?token|client.?secret|password|credential|authorization|cookie)$/i.test(item)
        if (flag) { redactNextArgument = true; return item }
        return item.replace(/((?:access.?token|refresh.?token|id.?token|client.?secret|password|credential|authorization|cookie)\s*[=:]\s*)[^\s,]+/gi, '$1[redacted]')
      }
      return redactSensitive(item)
    })
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, SECRET_KEY.test(key) ? '[redacted]' : redactSensitive(child)]))
  }
  return value
}

export function taskIsActive(task: TaskSnapshot | null): boolean {
  return task?.state === 'running' || task?.state === 'cancelling'
}

export function formatGuiError(cause: unknown, fallback: string): string {
  if (cause && typeof cause === 'object') {
    const payload = cause as { code?: unknown; message?: unknown }
    if (typeof payload.message === 'string' && payload.message.length > 0) {
      return typeof payload.code === 'string' && payload.code.length > 0
        ? `${payload.code}: ${payload.message}`
        : payload.message
    }
    if (cause instanceof Error && cause.message) return cause.message
  }
  if (typeof cause === 'string' && cause.length > 0) return cause
  return fallback
}

/** Never substitute a different game when the saved selection is missing. */
export function resolveSelectedInstance(entries: readonly LibraryEntry[], key: string | null): LibraryEntry | null {
  return key === null ? null : entries.find(entry => entry.key === key) ?? null
}
