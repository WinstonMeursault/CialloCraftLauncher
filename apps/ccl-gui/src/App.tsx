import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { boundLogLines, boundLogText } from './types'
import type {
  AccountListing, Bootstrap, GameVersion, ImportPreview, ImportRequest, Language,
  LaunchRequest, LibraryEntry, LoaderVersion, Preferences, RunRecord, TaskSnapshot, Theme,
} from './types'
import { acceptTaskSnapshot, formatGuiError, retainBusyEntries, taskIsActive } from './types'
import { getMessages } from './i18n'
import { InstanceDetails, Discover, ResultView } from './Advanced'
import { MaintenancePanel } from './Maintenance'
import { AccountsPanel } from './AccountsPanel'
import { ModalFrame } from './Dialog'
import { ProgressDialog } from './ProgressDialog'
import { LauncherLogs } from './LauncherLogs'

type Page = 'library' | 'discover' | 'accounts' | 'logs' | 'settings'
type Modal = 'create' | 'import' | null
type IconName = 'library' | 'compass' | 'user' | 'logs' | 'settings' | 'folder' | 'search'
type LaunchForm = { accountId: string; java: string; memory: string; autoInstallJava: boolean; dryRun: boolean }

const LOADERS = ['fabric', 'quilt', 'legacyfabric', 'optifine', 'forge', 'neoforge'] as const

function isDesktopRuntime() { return '__TAURI_INTERNALS__' in window }
function errorMessage(cause: unknown, fallback: string) { return formatGuiError(cause, fallback) }

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}
function summaryString(summary: unknown, key: string): string | null {
  const value = asRecord(summary)?.[key]
  return typeof value === 'string' && value.length ? value : null
}
function importIdFromPath(path: string) {
  const filename = path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') ?? 'imported-instance'
  return filename.trim() || 'imported-instance'
}
function logTail(text: string, lines: number) { return text.split(/\r?\n/).slice(-lines).join('\n') }
function isTerminal(task: TaskSnapshot) { return ['succeeded', 'failed', 'cancelled'].includes(task.state) }

function Icon({ name }: { name: IconName }) {
  const common = { fill: 'none', stroke: 'currentColor', strokeWidth: 1.6, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const }
  const paths = {
    library: <><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></>,
    compass: <><circle cx="12" cy="12" r="9"/><path d="m16 8-3 5-5 3 3-5z"/></>,
    user: <><circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/></>,
    tasks: <><rect x="5" y="4" width="14" height="17" rx="2"/><path d="M9 3h6v4H9zM9 12h6M9 16h4"/></>,
    logs: <><path d="M6 3h9l4 4v14H6zM9 11h7M9 15h7M9 18h4"/></>,
    settings: <><circle cx="12" cy="12" r="4"/><path d="M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2m12-12-2 2"/></>,
    folder: <><path d="M3 6a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></>,
    search: <><circle cx="10.8" cy="10.8" r="6.8"/><path d="m16 16 4.5 4.5"/></>,
  }
  return <svg aria-hidden="true" viewBox="0 0 24 24" {...common}>{paths[name]}</svg>
}

export default function App() {
  const desktop = useMemo(isDesktopRuntime, [])
  const [page, setPage] = useState<Page>('library')
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null)
  const bootstrapRef = useRef<Bootstrap | null>(null)
  bootstrapRef.current = bootstrap
  const [task, setTask] = useState<TaskSnapshot | null>(null)
  const taskCursor = useRef<TaskSnapshot | null>(null)
  const [launchPending, setLaunchPending] = useState(false)
  const [accountListing, setAccountListing] = useState<AccountListing>({ accounts: [], selected: null })
  const [pageNotice, setPageNotice] = useState<string | null>(null)
  const [theme, setTheme] = useState<Theme>('system')
  const [language, setLanguage] = useState<Language>('zh-CN')
  const languageRef = useRef(language)
  languageRef.current = language
  const [savedPreferences, setSavedPreferences] = useState<Preferences | null>(null)
  const [loading, setLoading] = useState(true)
  const [working, setWorking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [currentLaunchKey, setCurrentLaunchKey] = useState<string | null>(null)
  const [modal, setModal] = useState<Modal>(null)
  const [versions, setVersions] = useState<GameVersion[]>([])
  const [versionsLoading, setVersionsLoading] = useState(false)
  const [versionId, setVersionId] = useState('')
  const [instanceId, setInstanceId] = useState('')
  const [loader, setLoader] = useState('')
  const [loaderVersions, setLoaderVersions] = useState<LoaderVersion[]>([])
  const [loaderLoading, setLoaderLoading] = useState(false)
  const loaderRequest = useRef(0)
  const [loaderVersion, setLoaderVersion] = useState('')
  const [versionKind, setVersionKind] = useState('all')
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null)
  const [importId, setImportId] = useState('')
  const [includeOptional, setIncludeOptional] = useState(false)
  const [replaceModified, setReplaceModified] = useState(false)
  const [updateExisting, setUpdateExisting] = useState(false)
  const [allowedHosts, setAllowedHosts] = useState<string[]>([])
  const [manualHosts, setManualHosts] = useState('')
  const [launchForm, setLaunchForm] = useState<LaunchForm>({ accountId: '', java: '', memory: '', autoInstallJava: true, dryRun: false })
  const [offlineDraft, setOfflineDraft] = useState(false)
  const [pendingCancel, setPendingCancel] = useState<TaskSnapshot | null>(null)
  const [progressVisible, setProgressVisible] = useState(false)
  const [logSource, setLogSource] = useState<'game' | 'launcher'>('game')
  const [runs, setRuns] = useState<RunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [runLog, setRunLog] = useState('')
  const [logLoading, setLogLoading] = useState(false)
  const logReadSequence = useRef(0)
  const runListSequence = useRef(0)

  const t = getMessages(language)
  const hasUnsavedPreferences = !savedPreferences || theme !== savedPreferences.theme || language !== savedPreferences.language
  const activeTask = taskIsActive(task) ? task : null
  const liveLaunchActive = task?.kind === 'launch' && taskIsActive(task)
  const activeLaunch = liveLaunchActive ? task : null
  const pauseSavedLogs = launchPending || liveLaunchActive

  const captureError = useCallback((cause: unknown) => {
    setError(errorMessage(cause, getMessages(languageRef.current).unknownError))
    if (isDesktopRuntime()) void invoke('frontend_diagnostic', { event: 'operation_error' }).catch(() => {})
  }, [])

  const refreshBootstrap = useCallback(async () => {
    try {
      const result = await invoke<Bootstrap>('bootstrap')
      if (!result.setup_required) {
        const merged = bootstrapRef.current ? retainBusyEntries(bootstrapRef.current, result) : result
        setBootstrap(merged)
        setSelectedKey(current => merged.entries.some(entry => entry.key === current) ? current : merged.entries[0]?.key ?? null)
      }
    } catch (cause) { captureError(cause) }
  }, [captureError])

  const applyTask = useCallback((incoming: TaskSnapshot) => {
    const accepted = acceptTaskSnapshot(taskCursor.current, incoming)
    if (accepted === taskCursor.current) return
    const bounded = accepted ? { ...accepted, logs: boundLogLines(accepted.logs, 100, 256 * 1024) } : null
    taskCursor.current = bounded
    setTask(bounded)
    if (bounded?.kind === 'launch') setLaunchPending(false)
  }, [])

  const progressCursor = useRef<{ id: number; state: string; pid: number | null } | null>(null)
  useEffect(() => {
    if (!task) return
    const previous = progressCursor.current
    if (!previous || previous.id !== task.id || (previous.state !== 'failed' && task.state === 'failed')) setProgressVisible(true)
    if (task.kind === 'launch' && taskIsActive(task) && task.pid !== null && previous?.pid !== task.pid) setProgressVisible(false)
    progressCursor.current = { id: task.id, state: task.state, pid: task.pid }
  }, [task?.id, task?.state, task?.kind, task?.pid])

  const loadAuxiliary = useCallback(async () => {
    const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
    if (snapshot) applyTask(snapshot)
    else {
      taskCursor.current = null
      setTask(null)
    }
  }, [applyTask])

  const refreshTaskState = useCallback(async () => {
    try {
      const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
      if (snapshot) applyTask(snapshot)
      else { taskCursor.current = null; setTask(null) }
    } catch (cause) { captureError(cause) }
  }, [applyTask, captureError])

  // Event delivery can pause in an occluded WebView. Reconcile only while work
  // is active, with one in-flight read and a slower cadence for a running game.
  useEffect(() => {
    if (!desktop || !activeTask) return
    let disposed = false, reading = false
    const reconcile = async () => {
      if (reading) return
      reading = true
      try {
        const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
        if (!disposed && snapshot) applyTask(snapshot)
      } catch (cause) { if (!disposed) captureError(cause) }
      finally { reading = false }
    }
    const timer = window.setInterval(() => { void reconcile() }, activeTask.kind === 'launch' && activeTask.pid !== null ? 5000 : 1000)
    return () => { disposed = true; clearInterval(timer) }
  }, [desktop, activeTask?.id, activeTask?.pid, applyTask, captureError])

  const loadBootstrap = useCallback(async () => {
    if (!desktop) { setLoading(false); return }
    setLoading(true); setError(null)
    try {
      const result = await invoke<Bootstrap>('bootstrap')
      setBootstrap(result)
      setTheme(result.preferences.theme); setLanguage(result.preferences.language)
      setSavedPreferences(result.preferences); setOfflineDraft(result.launch_defaults.network_offline)
      setLaunchForm(current => ({ ...current, java: result.launch_defaults.java ?? '', memory: String(result.launch_defaults.max_memory_mb), autoInstallJava: result.launch_defaults.auto_install_java }))
      setSelectedKey(current => result.entries.some(entry => entry.key === current) ? current : result.entries[0]?.key ?? null)
      if (!result.setup_required) await loadAuxiliary()
    } catch (cause) { captureError(cause) }
    finally { setLoading(false) }
  }, [desktop, captureError, loadAuxiliary])

  useEffect(() => {
    if (!desktop) { setLoading(false); return }
    let disposed = false
    let unlisten: (() => void) | undefined
    let pendingSnapshot: TaskSnapshot | null = null
    let publishTimer: ReturnType<typeof setTimeout> | undefined
    const publish = (incoming: TaskSnapshot) => {
      pendingSnapshot = incoming
      if (document.hidden) return
      if (isTerminal(incoming)) {
        if (publishTimer) clearTimeout(publishTimer)
        publishTimer = undefined
        pendingSnapshot = null
        applyTask(incoming)
        return
      }
      if (publishTimer) return
      publishTimer = setTimeout(() => {
        publishTimer = undefined
        const latest = pendingSnapshot
        pendingSnapshot = null
        if (!disposed && !document.hidden && latest) applyTask(latest)
      }, 100)
    }
    void (async () => {
      try {
        const stopListening = await listen<TaskSnapshot>('gui-task', event => { if (!disposed) publish(event.payload) })
        if (disposed) { stopListening(); return }
        unlisten = stopListening
        await loadBootstrap()
      } catch (cause) { if (!disposed) { captureError(cause); setLoading(false) } }
    })()
    const onVisibility = () => {
      void invoke('set_visibility', { visible: !document.hidden }).then(async () => {
        if (!document.hidden) {
          const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
          if (disposed) return
          if (publishTimer) clearTimeout(publishTimer)
          publishTimer = undefined
          const latest = snapshot ?? pendingSnapshot
          pendingSnapshot = null
          if (latest) applyTask(latest)
          else { taskCursor.current = null; setTask(null) }
        }
      }).catch(captureError)
    }
    document.addEventListener('visibilitychange', onVisibility)
    onVisibility()
    return () => { disposed = true; if (publishTimer) clearTimeout(publishTimer); unlisten?.(); document.removeEventListener('visibilitychange', onVisibility) }
  }, [desktop, loadBootstrap, applyTask, captureError])


  const dropState = useRef({working, activeTask, bootstrap})
  dropState.current = {working, activeTask, bootstrap}
  useEffect(() => {
    if (!desktop) return
    let disposed=false
    const stops: (()=>void)[]=[]
    void (async()=>{
      const drop=await listen<string>('ccl-archive-drop', event=>{
        const state=dropState.current
        if (state.working || state.activeTask || !state.bootstrap || state.bootstrap.setup_required) {
          captureError(languageRef.current==='zh-CN'?'请先完成目录设置并等待当前任务结束，再拖入整合包。':'Finish setup and wait for the current task before dropping a pack.')
          return
        }
        setWorking(true);setError(null);setModal('import');setImportPreview(null)
        setIncludeOptional(false);setReplaceModified(false);setUpdateExisting(false);setAllowedHosts([]);setManualHosts('')
        void invoke<ImportPreview>('inspect_dropped',{archive:event.payload}).then(preview=>{
          if(disposed)return
          setImportPreview(preview);setImportId(importIdFromPath(preview.path))
        }).catch(captureError).finally(()=>{if(!disposed)setWorking(false)})
      })
      if(disposed){drop();return}stops.push(drop)
      const error=await listen<string>('ccl-drop-error',()=>captureError(languageRef.current==='zh-CN'?'每次请拖入一个 .zip 或 .mrpack 整合包。':'Drop one .zip or .mrpack archive at a time.'))
      if(disposed){error();return}stops.push(error)
    })().catch(captureError)
    return()=>{disposed=true;stops.forEach(stop=>stop())}
  },[desktop,captureError])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const update = () => document.documentElement.dataset.theme = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme
    update(); media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [theme])

  useEffect(() => {
    if (!desktop) return
    const reportError = () => { void invoke('frontend_diagnostic', { event: 'render_error' }).catch(() => {}) }
    const reportRejection = () => { void invoke('frontend_diagnostic', { event: 'unhandled_rejection' }).catch(() => {}) }
    window.addEventListener('error', reportError); window.addEventListener('unhandledrejection', reportRejection)
    return () => { window.removeEventListener('error', reportError); window.removeEventListener('unhandledrejection', reportRejection) }
  }, [desktop])
  useEffect(() => {
    if (desktop) void invoke('frontend_diagnostic', { event: 'navigation' }).catch(() => {})
  }, [desktop, page])
  const guiReadySent = useRef(false)
  const hasRenderedBootstrap = !!bootstrap
  useEffect(() => {
    if (!desktop || loading || !hasRenderedBootstrap || guiReadySent.current) return
    const frame = requestAnimationFrame(() => { guiReadySent.current = true; void invoke('gui_ready').catch(captureError) })
    return () => cancelAnimationFrame(frame)
  }, [desktop, loading, hasRenderedBootstrap, captureError])

  const entries = (bootstrap?.entries ?? []).filter(entry => {
    const displayName = entry.display_name
    const haystack = `${displayName} ${entry.id} ${entry.minecraft} ${entry.loader ?? ''} ${entry.game_directory}`.toLocaleLowerCase()
    return haystack.includes(query.toLocaleLowerCase().trim())
  })
  const selected = (bootstrap?.entries ?? []).find(entry => entry.key === selectedKey) ?? entries[0] ?? null

  const startTask = async (operation: () => Promise<TaskSnapshot>) => {
    if (working || activeTask) return null
    setWorking(true); setError(null); setPageNotice(null)
    try {
      const result = await operation()
      applyTask(result)
      setProgressVisible(true)
      return result
    } catch (cause) { captureError(cause); return null }
    finally { setWorking(false) }
  }

  const initializeRoot = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Bootstrap>('initialize_root')
      setBootstrap(result); setSelectedKey(result.entries[0]?.key ?? null)
      if (!result.setup_required) await loadAuxiliary()
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const chooseRoot = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Bootstrap | null>('select_root')
      if (result) {
        setBootstrap(result); setSelectedKey(result.entries[0]?.key ?? null)
        if (!result.setup_required) await loadAuxiliary()
      }
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const openCreate = async () => {
    setModal('create'); setVersions([]); setVersionId(''); setInstanceId(''); setLoader(''); setLoaderVersions([]); setLoaderVersion(''); setVersionKind('all')
    loaderRequest.current += 1; setLoaderLoading(false)
    setVersionsLoading(true); setError(null)
    try {
      const items = await invoke<GameVersion[]>('list_versions')
      setVersions(items)
      setVersionId(items[0]?.id ?? '')
    } catch (cause) { captureError(cause) }
    finally { setVersionsLoading(false) }
  }

  const changeLoader = async (nextLoader: string, minecraft = versionId) => {
    const sequence = ++loaderRequest.current
    setLoader(nextLoader); setLoaderVersions([]); setLoaderVersion('')
    if (!nextLoader || !minecraft) { setLoaderLoading(false); return }
    setLoaderLoading(true); setError(null)
    try {
      const items = await invoke<LoaderVersion[]>('list_loaders', { minecraft, loader: nextLoader })
      if (sequence === loaderRequest.current) { setLoaderVersions(items); setLoaderVersion(items[0]?.version ?? '') }
    } catch (cause) { if (sequence === loaderRequest.current) captureError(cause) }
    finally { if (sequence === loaderRequest.current) setLoaderLoading(false) }
  }
  const chooseVersion = (version: string) => {
    setVersionId(version)
    if (loader) void changeLoader(loader, version)
  }

  const installInstance = async (event: FormEvent) => {
    event.preventDefault()
    const id = instanceId.trim()
    if (!id || !versionId || working || activeTask || (loader && !loaderVersion)) return
    const request = { id, minecraft: versionId, loader: loader || null, loader_version: loaderVersion || null }
    const result = await startTask(() => invoke<TaskSnapshot>('install_instance', { request }))
    if (result) setModal(null)
  }

  const inspectImport = async () => {
    if (working || activeTask) return
    setWorking(true)
    setError(null)
    try {
      const preview = await invoke<ImportPreview | null>('inspect_import')
      if (!preview) return
      setImportPreview(preview); setImportId(importIdFromPath(preview.path)); setAllowedHosts([]); setManualHosts(''); setIncludeOptional(false); setReplaceModified(false)
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const importInstance = async (event: FormEvent) => {
    event.preventDefault()
    if (!importPreview || !importId.trim() || working || activeTask) return
    const requestedHosts = [...new Set([...allowedHosts, ...manualHosts.split(/[\n,]/).map(host => host.trim()).filter(Boolean)])]
    if (updateExisting && !window.confirm(language==='zh-CN' ? `更新实例 ${importId.trim()}？已有整合包文件可能被替换。` : `Update instance ${importId.trim()}? Existing pack files may be replaced.`)) return
    const request: ImportRequest = { update_existing: updateExisting, id: importId.trim(), archive: importPreview.path, include_optional: includeOptional, replace_modified: replaceModified, allowed_hosts: requestedHosts }
    const result = await startTask(() => invoke<TaskSnapshot>('import_instance', { request }))
    if (result) { setModal(null); setImportPreview(null) }
  }

  const loadAccounts = useCallback(async () => {
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_list')
      setAccountListing(listing)
      setLaunchForm(current => ({ ...current, accountId: listing.selected || '' }))
    }
    catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }, [captureError])

  const enterPage = (next: Page) => {
    setPage(next); setPageNotice(null)
    if (next === 'accounts' && !bootstrap?.setup_required) void loadAccounts()
    if (next === 'logs' && !bootstrap?.setup_required) void refreshTaskState()
  }

  const accountSelect = async (id: string) => {
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_select', { id })
      setAccountListing(listing); setLaunchForm(current => ({ ...current, accountId: id }))
    }
    catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }
  const accountLogout = async (id: string) => {
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_logout', { id })
      setAccountListing(listing); setLaunchForm(current => ({ ...current, accountId: current.accountId === id ? listing.selected ?? '' : current.accountId }))
    }
    catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }
  const addOfflineAccount = async (username: string) => {
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_add_offline', { username })
      setAccountListing(listing)
      setLaunchForm(current => ({ ...current, accountId: listing.selected ?? '' }))
    } catch (cause) { captureError(cause); throw cause }
    finally { setWorking(false) }
  }
  useEffect(() => {
    if (bootstrap && !bootstrap.setup_required) void loadAccounts()
  }, [bootstrap?.config_path, bootstrap?.setup_required, loadAccounts])

  const savePreferences = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Preferences>('save_preferences', { preferences: { theme, language } })
      setTheme(result.theme); setLanguage(result.language); setSavedPreferences(result); setPageNotice(t.saved)
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const restoreHidden = async () => {
    if (!bootstrap || bootstrap.setup_required || working || activeTask) return
    setWorking(true); setError(null); setPageNotice(null)
    try {
      await invoke('library_update', { request: { operation: 'restore_all' } })
      await refreshBootstrap()
      setPageNotice(t.hiddenRestored)
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }
  const saveNetworkOffline = async () => {
    if (!bootstrap || activeTask) return
    setWorking(true); setError(null)
    try {
      const result = await invoke<Bootstrap>('set_network_offline', { offline: offlineDraft })
      setBootstrap(result); setOfflineDraft(result.launch_defaults.network_offline); setPageNotice(t.saved)
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const launch = async (event: FormEvent) => {
    event.preventDefault()
    if (!selected || working || activeTask) return
    const memoryValue = launchForm.memory.trim() ? Number(launchForm.memory) : null
    if (memoryValue !== null && (!Number.isSafeInteger(memoryValue) || memoryValue <= 0 || memoryValue > 4_294_967_295)) return
    const account = accountListing.accounts.find(item => item.id === launchForm.accountId)
    if (!account) { enterPage('accounts'); return }
    const request: LaunchRequest = {
      key: selected.key,
      identity: account.account_type === 'offline'
        ? { kind: 'offline', username: account.username, account_id: account.id }
        : { kind: 'microsoft', username: null, account_id: account.id },
      java: launchForm.java.trim() || null,
      memory_mb: memoryValue,
      auto_install_java: launchForm.autoInstallJava,
      dry_run: launchForm.dryRun,
    }
    setCurrentLaunchKey(selected.key); setLaunchPending(true)
    const result = await startTask(() => invoke<TaskSnapshot>('launch_game', { request }))
    if (!result) { setCurrentLaunchKey(null); setLaunchPending(false) }
  }

  const confirmCancel = async () => {
    if (!pendingCancel) return
    setWorking(true); setError(null)
    try { applyTask(await invoke<TaskSnapshot>('cancel_task', { id: pendingCancel.id })); setPendingCancel(null) }
    catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const readLog = useCallback(async (key: string, runId: string | null, lines: number) => {
    const sequence = ++logReadSequence.current
    setLogLoading(true)
    try {
      const result = await invoke<{ text: string }>('read_run_log', { key, runId, lines })
      if (sequence === logReadSequence.current) setRunLog(boundLogText(logTail(result.text, Math.max(1, Math.min(lines, 1000))), 1000))
    } catch (cause) {
      const payload = asRecord(cause)
      if (sequence === logReadSequence.current) {
        if (payload?.code === 'log_read' && typeof payload.message === 'string' && /no launch logs found/i.test(payload.message)) setRunLog('')
        else captureError(cause)
      }
    }
    finally { if (sequence === logReadSequence.current) setLogLoading(false) }
  }, [captureError])

  const loadRuns = useCallback(async (key: string, preferred: string | null = null) => {
    const sequence = ++runListSequence.current
    try {
      const result = await invoke<RunRecord[]>('log_runs', { key })
      if (sequence !== runListSequence.current) return
      const bounded = result.slice(0, 100)
      setRuns(bounded)
      const selectedRun = preferred && bounded.some(run => run.run_id === preferred) ? preferred : bounded[0]?.run_id ?? null
      setSelectedRunId(selectedRun)
      if (selectedRun) await readLog(key, selectedRun, 500)
      else setRunLog('')
    } catch (cause) { if (sequence === runListSequence.current) captureError(cause) }
  }, [captureError, readLog])

  const previousLogKey = useRef<string | null>(selectedKey)
  useEffect(() => {
    if (previousLogKey.current === selectedKey) return
    previousLogKey.current = selectedKey
    setRuns([]); setSelectedRunId(null); setRunLog('')
  }, [selectedKey])

  useEffect(() => {
    if (page !== 'logs' || !selectedKey || pauseSavedLogs) return
    void loadRuns(selectedKey)
  }, [page, selectedKey, pauseSavedLogs, loadRuns])

  useEffect(() => {
    if (!task || !isTerminal(task)) return
    if (['instance','online'].includes(task.kind) || task.kind.includes('install') || task.kind.includes('import')) void refreshBootstrap()
    if (['login', 'refresh'].includes(task.kind)) void loadAccounts()
  }, [task?.id, task?.revision, task?.kind, task?.state, refreshBootstrap, loadAccounts])

  const activePageLabel = page === 'library' ? t.library : page === 'discover' ? t.discover : page === 'accounts' ? t.accounts : page === 'logs' ? t.logs : t.settings
  if (!desktop) return <div className="connection-card"><div className="connection-symbol">!</div><h1>{t.notConnected}</h1><p>{t.notConnectedCopy}</p></div>

  return <div className="shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark" aria-hidden="true">✧</div><div><strong>CCL</strong><small>{t.brand}</small></div></div>
      <nav aria-label={t.brand}>
        <NavItem icon="library" label={t.library} active={page === 'library'} onClick={() => enterPage('library')} />
        <NavItem icon="compass" label={t.discover} active={page === 'discover'} disabled={!!bootstrap?.setup_required} onClick={() => enterPage('discover')} />
        <NavItem icon="user" label={t.accounts} active={page === 'accounts'} disabled={!!bootstrap?.setup_required} unavailableLabel={t.setupTitle} onClick={() => enterPage('accounts')} />
        <NavItem icon="logs" label={t.logs} active={page === 'logs'} onClick={() => enterPage('logs')} />
      </nav>
      <div className="sidebar-bottom"><NavItem icon="settings" label={t.settings} active={page === 'settings'} onClick={() => enterPage('settings')} /></div>
    </aside>

    <main className="main">
      <header className="topline"><span className="crumb">{t.brand} <span aria-hidden="true">/</span> {activePageLabel}</span></header>
      {error && <div className="alert" role="alert"><strong>{t.error}</strong><span>{error}</span><button className="icon-button" aria-label="Dismiss" onClick={() => setError(null)}>×</button></div>}
      {pageNotice && <div className="notice" role="status">{pageNotice}<button className="icon-button" aria-label="Dismiss" onClick={() => setPageNotice(null)}>×</button></div>}
      {page === 'logs' && (!bootstrap || bootstrap.setup_required) ? <LauncherLogs language={language}/> : loading || !bootstrap ? <><div className="loading" role="status">{loading ? t.loading : t.error}</div>{!loading && page === 'settings' && <SettingsPage t={t} theme={theme} language={language} setTheme={setTheme} setLanguage={setLanguage} savePreferences={savePreferences} working={working} hasUnsaved={hasUnsavedPreferences} saved={false} offlineDraft={offlineDraft} setOfflineDraft={setOfflineDraft} offlineSaved={false} saveNetworkOffline={saveNetworkOffline} busy={!!activeTask} root="" chooseRoot={chooseRoot} setupOnly />}{!loading && page !== 'settings' && <MaintenancePanel language={language} busy={working} entry={null} onChanged={() => void loadBootstrap()} onTask={applyTask} />}</> : bootstrap.setup_required
        ? page === 'settings' ? <SettingsPage t={t} theme={theme} language={language} setTheme={setTheme} setLanguage={setLanguage} savePreferences={savePreferences} working={working} hasUnsaved={hasUnsavedPreferences} saved={!!savedPreferences && !hasUnsavedPreferences} offlineDraft={offlineDraft} setOfflineDraft={setOfflineDraft} offlineSaved={false} saveNetworkOffline={saveNetworkOffline} busy={!!activeTask} root={bootstrap.root} chooseRoot={chooseRoot} setupOnly /> : <SetupPage t={t} bootstrap={bootstrap} working={working} chooseRoot={chooseRoot} initializeRoot={initializeRoot} />
        : <>
          {page === 'library' && <LibraryPage t={t} entries={entries} selected={selected} bootstrap={bootstrap} query={query} setQuery={setQuery} setSelectedKey={setSelectedKey} openCreate={openCreate} openImport={() => { setModal('import'); setImportPreview(null); setImportId(''); setIncludeOptional(false); setReplaceModified(false);setUpdateExisting(false); setAllowedHosts([]); setManualHosts(''); void inspectImport() }} launchForm={launchForm} setLaunchForm={setLaunchForm} launch={launch} activeTask={!!activeTask} working={working} accountListing={accountListing} selectAccount={accountSelect} openAccounts={() => enterPage('accounts')} />}
          {page === 'library' && <InstanceDetails language={language} entry={selected} busy={working || !!activeTask} onTask={incoming => { applyTask(incoming); setProgressVisible(true) }} onChanged={() => void refreshBootstrap()} />}
          {page === 'discover' && <Discover entries={bootstrap.entries} onSelectEntry={setSelectedKey} language={language} entry={selected} busy={working || !!activeTask} onTask={incoming => { applyTask(incoming); setProgressVisible(true) }} onChanged={() => void refreshBootstrap()} />}
          {page === 'accounts' && <AccountsPanel language={language} accounts={accountListing.accounts} selected={accountListing.selected} disabled={working || !!activeTask} onAddOffline={addOfflineAccount} onSelect={accountSelect} onRemove={accountLogout} />}
          {page === 'logs' && <><div className="segmented log-source-switch"><button className={logSource === 'game' ? 'selected' : ''} onClick={() => setLogSource('game')}>{language === 'zh-CN' ? '游戏日志' : 'Game logs'}</button><button className={logSource === 'launcher' ? 'selected' : ''} onClick={() => setLogSource('launcher')}>{language === 'zh-CN' ? '启动器日志' : 'Launcher logs'}</button></div>{logSource === 'launcher' ? <LauncherLogs language={language} /> : <LogsPage t={t} entries={bootstrap.entries} selectedKey={selectedKey} setSelectedKey={setSelectedKey} runs={runs} selectedRunId={selectedRunId} selectRun={(runId) => { setSelectedRunId(runId); if (selectedKey) void readLog(selectedKey, runId, 500) }} runLog={runLog} logLoading={logLoading} refresh={() => selectedKey && void loadRuns(selectedKey, selectedRunId)} liveTask={activeLaunch} logsPaused={pauseSavedLogs} liveEntry={bootstrap.entries.find(entry => entry.key === currentLaunchKey)?.id ?? null} />}</>}
          {page === 'settings' && <SettingsPage t={t} theme={theme} language={language} setTheme={setTheme} setLanguage={setLanguage} savePreferences={savePreferences} working={working} hasUnsaved={hasUnsavedPreferences} saved={!!savedPreferences && !hasUnsavedPreferences} offlineDraft={offlineDraft} setOfflineDraft={setOfflineDraft} offlineSaved={bootstrap.launch_defaults.network_offline} saveNetworkOffline={saveNetworkOffline} busy={!!activeTask} root={bootstrap.root} chooseRoot={chooseRoot} hiddenEntries={bootstrap.hidden_entries} restoreHidden={restoreHidden} />}
          {page === 'settings' && <MaintenancePanel language={language} busy={working || !!activeTask} entry={selected} onChanged={() => void loadBootstrap()} onTask={incoming => { applyTask(incoming); setProgressVisible(true) }} />}
        </>}
    </main>

    {task && !modal && !pendingCancel && <ProgressDialog task={task} language={language} visible={progressVisible} onVisibility={setProgressVisible} onCancel={() => setPendingCancel(task)} onLogs={() => { setPage('logs'); setLogSource('game'); setProgressVisible(false) }} />}
    {modal === 'create' && <CreateModal t={t} versions={versions} versionsLoading={versionsLoading} versionId={versionId} setVersionId={chooseVersion} versionKind={versionKind} setVersionKind={setVersionKind} instanceId={instanceId} setInstanceId={setInstanceId} loader={loader} changeLoader={changeLoader} loaderLoading={loaderLoading} loaderVersions={loaderVersions} loaderVersion={loaderVersion} setLoaderVersion={setLoaderVersion} working={working || !!activeTask} close={() => setModal(null)} submit={installInstance} />}
    {modal === 'import' && <ImportModal language={language} updateExisting={updateExisting} setUpdateExisting={setUpdateExisting} t={t} preview={importPreview} inspect={inspectImport} id={importId} setId={setImportId} includeOptional={includeOptional} setIncludeOptional={setIncludeOptional} replaceModified={replaceModified} setReplaceModified={setReplaceModified} allowedHosts={allowedHosts} setAllowedHosts={setAllowedHosts} manualHosts={manualHosts} setManualHosts={setManualHosts} working={working || !!activeTask} close={() => setModal(null)} submit={importInstance} />}
    {pendingCancel && <ConfirmDialog t={t} task={pendingCancel} working={working} cancel={() => setPendingCancel(null)} confirm={confirmCancel} />}
  </div>
}

function NavItem({ icon, label, active = false, disabled = false, unavailableLabel, onClick }: { icon: IconName; label: string; active?: boolean; disabled?: boolean; unavailableLabel?: string; onClick?: () => void }) {
  return <button className={`nav-item${active ? ' active' : ''}${disabled ? ' disabled' : ''}`} onClick={onClick} disabled={disabled} title={disabled ? `${label} · ${unavailableLabel ?? 'Not yet available'}` : label} aria-current={active ? 'page' : undefined}>
    <Icon name={icon}/><span>{label}</span>{disabled && <span className="nav-disabled">{unavailableLabel}</span>}{active && <span className="active-dot" aria-hidden="true"/>}
  </button>
}

function SetupPage({ t, bootstrap, working, chooseRoot, initializeRoot }: { t: ReturnType<typeof getMessages>; bootstrap: Bootstrap; working: boolean; chooseRoot: () => void; initializeRoot: () => void }) {
  return <section className="setup-card">
    <span className="eyebrow">{t.root}</span><h1>{t.setupTitle}</h1><p>{t.setupCopy}</p>
    <div className="setup-path"><span className="root-info-icon"><Icon name="folder"/></span><div><strong>{bootstrap.root_exists ? t.setupFolderExists : t.setupFolderMissing}</strong><code>{bootstrap.root}</code></div></div>
    <div className="setup-actions"><button className="button secondary" onClick={chooseRoot} disabled={working}><Icon name="folder"/>{t.setupChoose}</button><button className="button primary" onClick={initializeRoot} disabled={working}>{working ? t.initializing : bootstrap.root_exists ? t.setupExisting : t.setupNew}</button></div>
  </section>
}

function LibraryPage({ t, entries, selected, bootstrap, query, setQuery, setSelectedKey, openCreate, openImport, launchForm, setLaunchForm, launch, activeTask, working, accountListing, selectAccount, openAccounts }: {
  t: ReturnType<typeof getMessages>; entries: LibraryEntry[]; selected: LibraryEntry | null; bootstrap: Bootstrap; query: string; setQuery: (value: string) => void; setSelectedKey: (value: string) => void;
  openCreate: () => void; openImport: () => void; launchForm: LaunchForm; setLaunchForm: (form: LaunchForm) => void; launch: (event: FormEvent) => void; activeTask: boolean; working: boolean; accountListing: AccountListing; selectAccount: (id: string) => void; openAccounts: () => void
}) {
  return <>
    <section className="hero"><div className="hero-copy"><div className="eyebrow">{t.welcome}</div><h1>{t.heading}</h1><p>{t.subheading}</p></div><Landscape/></section>
    <div className="section-heading"><div><h2>{t.instances} ({bootstrap.entries.length})</h2></div><div className="heading-actions"><button className="button secondary" onClick={openImport} disabled={working || activeTask}>{t.importPack}</button><button className="button primary" onClick={openCreate} disabled={working || activeTask}>＋ {t.createInstance}</button></div></div>
    <div className="library-layout">
      <section className="library-list" aria-label={t.instances}>
        <label className="search-field"><Icon name="search"/><input value={query} onChange={event => setQuery(event.target.value)} placeholder={t.search}/></label>
        <div className="entry-list">{entries.map((entry, index) => <EntryCard key={entry.key} entry={entry} selected={entry.key === selected?.key} index={index} t={t} onClick={() => setSelectedKey(entry.key)}/>)}{entries.length === 0 && <div className="empty-state"><div>⌕</div><strong>{t.empty}</strong><p>{t.emptyHint}</p></div>}</div>
        <div className="root-info"><span className="root-info-icon"><Icon name="folder"/></span><div><strong>{t.root}</strong><span title={bootstrap.root}>{bootstrap.root}</span></div></div>
      </section>
      <section className="detail-card" aria-label={t.selected}>
        {selected ? <>
          <div className="detail-head"><span>{t.selected}</span></div>
          <h2>{selected.display_name}</h2><p className="detail-subtitle">Minecraft {selected.minecraft}</p>
          <dl className="metadata"><Meta label={t.id} value={selected.id}/><Meta label={t.gameVersion} value={selected.minecraft}/><Meta label={t.softwareType} value={selected.loader ?? t.vanillaSoftware}/></dl>
          <div className="path-box"><span>{t.gameDirectory}</span><code title={selected.game_directory}>{selected.game_directory}</code></div>
          <form className="launch-form" onSubmit={launch}>
            <label className="field"><span>{t.account}</span><select value={launchForm.accountId} onChange={event => selectAccount(event.target.value)} disabled={working || activeTask}><option value="">{t.selectAccount}</option>{accountListing.accounts.map(account => <option key={account.id} value={account.id}>{account.username} · {account.account_type === 'offline' ? t.offlineIdentity : 'Microsoft'}</option>)}</select></label>
            <button className="text-button" type="button" onClick={openAccounts}>{t.manageAccounts}</button>
            <details className="launch-options"><summary>{t.launchOptions}</summary><div className="launch-options-body"><label className="field"><span>{t.javaPath}</span><input value={launchForm.java} onChange={event => setLaunchForm({ ...launchForm, java: event.target.value })}/></label><label className="field"><span>{t.memory}</span><input inputMode="numeric" type="number" min="256" max="131072" step="1" value={launchForm.memory} onChange={event => setLaunchForm({ ...launchForm, memory: event.target.value })}/></label><label className="check-row"><input type="checkbox" checked={launchForm.autoInstallJava} onChange={event => setLaunchForm({ ...launchForm, autoInstallJava: event.target.checked })}/>{t.autoInstallJava}</label><label className="check-row"><input type="checkbox" checked={launchForm.dryRun} onChange={event => setLaunchForm({ ...launchForm, dryRun: event.target.checked })}/>{t.dryRun}</label></div></details>
            <button className="button primary launch-button" disabled={working || activeTask || !launchForm.accountId}>{working ? t.starting : t.launchGame}</button>
          </form>
        </> : <div className="empty-detail"><div className="empty-detail-mark">✧</div><h2>{t.empty}</h2><p>{t.emptyHint}</p></div>}
      </section>
    </div>
    <div className="root-footnote"><span>{t.rootPath}: <code>{bootstrap.root}</code></span><span>{t.configPath}: <code>{bootstrap.config_path}</code></span></div>
    {bootstrap.warnings.length > 0 && <section className="warnings"><strong>{t.warnings}</strong>{bootstrap.warnings.map((warning, i) => <p key={`${i}-${warning}`}>{warning}</p>)}</section>}
  </>
}

function EntryCard({ entry, selected, index, t, onClick }: { entry: LibraryEntry; selected: boolean; index: number; t: ReturnType<typeof getMessages>; onClick: () => void }) {
  const displayName = entry.display_name
  return <button className={`entry-card${selected ? ' selected' : ''}`} onClick={onClick} aria-pressed={selected}>
    <span className={`entry-tile tile-${index % 3}`} aria-hidden="true">{entry.loader ? '✧' : '▧'}</span>
    <span className="entry-copy"><strong>{displayName}</strong><span className="tags"><i>{entry.minecraft}</i><i>{entry.loader ?? t.vanillaSoftware}</i></span></span>{selected && <span className="selected-check" aria-label="Selected">✓</span>}
  </button>
}
function Meta({ label, value }: { label: string; value: string }) { return <div><dt>{label}</dt><dd>{value}</dd></div> }

function CreateModal({ t, versions, versionsLoading, versionId, setVersionId, versionKind, setVersionKind, instanceId, setInstanceId, loader, changeLoader, loaderLoading, loaderVersions, loaderVersion, setLoaderVersion, working, close, submit }: {
  t: ReturnType<typeof getMessages>; versions: GameVersion[]; versionsLoading: boolean; versionId: string; setVersionId: (value: string) => void; versionKind: string; setVersionKind: (value: string) => void; instanceId: string; setInstanceId: (value: string) => void;
  loader: string; changeLoader: (value: string, minecraft?: string) => void; loaderLoading: boolean; loaderVersions: LoaderVersion[]; loaderVersion: string; setLoaderVersion: (value: string) => void; working: boolean; close: () => void; submit: (event: FormEvent) => void
}) {
  const kinds = [...new Set(versions.map(version => version.kind))]
  const visibleVersions = versionKind === 'all' ? versions : versions.filter(version => version.kind === versionKind)
  return <ModalFrame closeLabel={t.close} title={t.newInstance} close={close}>
    <form className="modal-form" onSubmit={submit}>
      <label className="field"><span>{t.name}</span><input required value={instanceId} onChange={event => setInstanceId(event.target.value)} autoComplete="off"/></label>
      <label className="field"><span>{t.version}</span><select required value={versionId} disabled={versionsLoading || !versions.length} onChange={event => setVersionId(event.target.value)}><option value="">{versionsLoading ? t.loadingVersions : t.noVersions}</option>{visibleVersions.map(item => <option key={item.id} value={item.id}>{item.id} · {item.kind}</option>)}</select></label>
      <label className="field"><span>{t.versionKind}</span><select value={versionKind} onChange={event => { const kind = event.target.value; setVersionKind(kind); if (kind !== 'all' && !versions.some(item => item.id === versionId && item.kind === kind)) setVersionId(versions.find(item => item.kind === kind)?.id ?? '') }}><option value="all">{t.all}</option>{kinds.map(kind => <option key={kind} value={kind}>{kind}</option>)}</select></label>
      <label className="field"><span>{t.softwareType}</span><select value={loader} onChange={event => changeLoader(event.target.value)}><option value="">Vanilla</option>{LOADERS.map(item => <option key={item} value={item}>{item}</option>)}</select></label>
      {loader && <label className="field"><span>{t.loaderVersion}</span><select value={loaderVersion} disabled={loaderLoading || !loaderVersions.length} onChange={event => setLoaderVersion(event.target.value)}><option value="">{loaderLoading ? t.loadingVersions : t.noVersions}</option>{loaderVersions.map(item => <option key={item.version} value={item.version}>{item.version}{item.stable ? '' : ' · preview'}</option>)}</select></label>}
      <div className="modal-actions"><button type="button" className="button secondary" onClick={close}>{t.keepRunning}</button><button className="button primary" disabled={working || versionsLoading || !versionId || (!!loader && (!loaderVersion || loaderLoading))}>{working ? t.starting : t.install}</button></div>
    </form>
  </ModalFrame>
}

function ImportModal({ language, updateExisting, setUpdateExisting, t, preview, inspect, id, setId, includeOptional, setIncludeOptional, replaceModified, setReplaceModified, allowedHosts, setAllowedHosts, manualHosts, setManualHosts, working, close, submit }: {
  language: Language; updateExisting: boolean; setUpdateExisting:(value:boolean)=>void; t: ReturnType<typeof getMessages>; preview: ImportPreview | null; inspect: () => void; id: string; setId: (value: string) => void; includeOptional: boolean; setIncludeOptional: (value: boolean) => void; replaceModified: boolean; setReplaceModified: (value: boolean) => void; allowedHosts: string[]; setAllowedHosts: (value: string[]) => void; manualHosts: string; setManualHosts: (value: string) => void; working: boolean; close: () => void; submit: (event: FormEvent) => void
}) {
  const wrapper = asRecord(preview?.summary)
  const summary = asRecord(wrapper?.manifest) ?? wrapper
  const hosts = ['allowed_hosts', 'allowedHosts', 'hosts'].flatMap(key => {
    const values = [wrapper?.[key], summary?.[key]]
    return values.flatMap(value => Array.isArray(value) ? value : [])
  }).filter((item): item is string => typeof item === 'string')
  const shortSummary = ['format', 'name', 'version', 'minecraft', 'loader'].flatMap(key => {
    const value = summaryString(summary, key)
    return value ? [{ key, value }] : []
  })
  return <ModalFrame closeLabel={t.close} title={t.importPack} close={close}>
    <form className="modal-form" onSubmit={submit}>
      {!preview ? <div className="picker-state"><p>{t.inspectPack}</p><button type="button" className="button secondary" onClick={inspect}>{t.inspectPack}</button></div> : <>
        <div className="path-box"><span>{t.archive}</span><code title={preview.path}>{preview.path}</code></div>
        <label className="field"><span>{t.name}</span><input required value={id} onChange={event => setId(event.target.value)}/></label>
        <section className="summary-box"><strong>{t.packSummary}</strong>{shortSummary.map(item => <Meta key={item.key} label={item.key} value={item.value}/>)}<details><summary>{t.rawSummary}</summary><ResultView value={preview?.summary} language={language}/></details></section>
        <label className="check-row"><input type="checkbox" checked={includeOptional} onChange={event => setIncludeOptional(event.target.checked)}/>{t.includeOptional}</label>
        <label className="check-row"><input type="checkbox" checked={updateExisting} onChange={event => setUpdateExisting(event.target.checked)}/>{language==='zh-CN'?'更新已有实例（默认新建；更新前会确认）':'Update an existing instance (creates new by default; confirmation required)'}</label>
        <label className="check-row"><input type="checkbox" checked={replaceModified} onChange={event => setReplaceModified(event.target.checked)}/>{t.replaceModified}</label>
        {hosts.length > 0 && <fieldset className="host-list"><legend>{t.allowedHosts}</legend>{[...new Set(hosts)].map(host => <label className="check-row" key={host}><input type="checkbox" checked={allowedHosts.includes(host)} onChange={event => setAllowedHosts(event.target.checked ? [...allowedHosts, host] : allowedHosts.filter(item => item !== host))}/>{host}</label>)}</fieldset>}
        <label className="field"><span>{t.manualHosts}</span><textarea rows={2} value={manualHosts} onChange={event => setManualHosts(event.target.value)} placeholder="cdn.example.org"/></label>
        <button type="button" className="text-button" onClick={inspect}>{t.inspectAgain}</button>
      </>}
      <div className="modal-actions"><button type="button" className="button secondary" onClick={close}>{t.keepRunning}</button><button className="button primary" disabled={working || !preview || !id.trim()}>{working ? t.starting : t.import}</button></div>
    </form>
  </ModalFrame>
}

function LogsPage({ t, entries, selectedKey, setSelectedKey, runs, selectedRunId, selectRun, runLog, logLoading, refresh, liveTask, logsPaused, liveEntry }: { t: ReturnType<typeof getMessages>; entries: LibraryEntry[]; selectedKey: string | null; setSelectedKey: (key: string) => void; runs: RunRecord[]; selectedRunId: string | null; selectRun: (id: string | null) => void; runLog: string; logLoading: boolean; refresh: () => void; liveTask: TaskSnapshot | null; logsPaused: boolean; liveEntry: string | null }) {
  const selected = entries.find(entry => entry.key === selectedKey)
  const run = runs.find(item => item.run_id === selectedRunId) ?? runs[0] ?? null
  const live = liveTask?.logs.slice(-100) ?? []
  return <>
    <PageHeading eyebrow={t.logs} title={t.logsTitle} description={t.logSaved}/>
    {logsPaused && <section className="logs-panel live-launch-panel"><div className="panel-heading"><div><h2>{t.currentLaunch}</h2><p>{liveEntry ? `${t.selected}: ${liveEntry} · ` : ''}{liveTask ? `${liveTask.phase} · #${liveTask.id}` : t.starting}</p></div><span className="state-tag state-running">{liveTask ? t.state[liveTask.state] : t.state.running}</span></div><pre className="run-log">{live.length ? live.join('\n') : t.noLogs}</pre></section>}
    <section className={`logs-panel${logsPaused ? ' saved-logs-paused' : ''}`}><div className="log-controls"><label className="field"><span>{t.chooseEntry}</span><select value={selectedKey ?? ''} disabled={logsPaused} onChange={event => setSelectedKey(event.target.value)}><option value="">{t.chooseEntry}</option>{entries.map(entry => <option key={entry.key} value={entry.key}>{entry.id}</option>)}</select></label><label className="field"><span>{t.chooseRun}</span><select value={selectedRunId ?? ''} disabled={logsPaused || runs.length === 0} onChange={event => selectRun(event.target.value || null)}><option value="">{t.latestRun}</option>{runs.map(item => <option key={item.run_id} value={item.run_id}>{item.version} · {item.run_id}</option>)}</select></label><button className="button secondary" onClick={refresh} disabled={!selectedKey || logLoading || logsPaused}>{logLoading ? t.loading : t.refreshLogs}</button></div>
      {logsPaused && <p className="saved-logs-note">{t.savedPaused}</p>}
      {run && <dl className="run-metadata"><Meta label={t.gameVersion} value={run.version}/><Meta label={t.pid} value={run.pid === null ? '—' : String(run.pid)}/><Meta label={t.exitCode} value={run.exit_code === null ? '—' : String(run.exit_code)}/><Meta label={t.started} value={new Date(run.started_at_ms).toLocaleString()}/><Meta label={t.finished} value={run.finished_at_ms === null ? t.liveLog : new Date(run.finished_at_ms).toLocaleString()}/><Meta label={t.logFile} value={run.log_path}/></dl>}
      {!logsPaused && selected && runs.length === 0 && <p className="muted-copy">{t.noRuns}</p>}
      {!logsPaused && <pre className="run-log" aria-label={t.logsTitle}>{runLog || (logLoading ? t.loading : t.noRuns)}</pre>}
    </section>
  </>
}

function SettingsPage({ t, theme, language, setTheme, setLanguage, savePreferences, working, hasUnsaved, saved, offlineDraft, setOfflineDraft, offlineSaved, saveNetworkOffline, busy, root, chooseRoot, setupOnly = false, hiddenEntries = 0, restoreHidden }: {
  t: ReturnType<typeof getMessages>; theme: Theme; language: Language; setTheme: (theme: Theme) => void; setLanguage: (language: Language) => void; savePreferences: () => void; working: boolean; hasUnsaved: boolean; saved: boolean; offlineDraft: boolean; setOfflineDraft: (offline: boolean) => void; offlineSaved: boolean; saveNetworkOffline: () => void; busy: boolean; root: string; chooseRoot: () => void; setupOnly?: boolean; hiddenEntries?: number; restoreHidden?: () => void
}) {
  return <>
    <PageHeading eyebrow={t.settings} title={t.appearance} description={t.appearanceCopy}/>
    <section className="settings-card">
      <div className="setting-row"><div><h2>{t.theme}</h2><p>{t.appearanceCopy}</p></div><div className="segmented" role="group" aria-label={t.theme}>{(['system', 'light', 'dark'] as const).map(value => <button key={value} aria-pressed={theme === value} className={theme === value ? 'selected' : ''} onClick={() => setTheme(value)}>{value === 'system' ? t.system : value === 'light' ? t.light : t.dark}</button>)}</div></div>
      <div className="setting-row"><div><h2>{t.language}</h2><p>{t.language}</p></div><div className="segmented" role="group" aria-label={t.language}><button className={language === 'en' ? 'selected' : ''} aria-pressed={language === 'en'} onClick={() => setLanguage('en')}>{t.english}</button><button className={language === 'zh-CN' ? 'selected' : ''} aria-pressed={language === 'zh-CN'} onClick={() => setLanguage('zh-CN')}>{t.chinese}</button></div></div>
      <div className="settings-actions"><span role="status">{hasUnsaved ? t.unsaved : saved ? t.saved : ''}</span><button className="button primary" onClick={savePreferences} disabled={working || !hasUnsaved}>{working ? t.starting : t.save}</button></div>
      {!setupOnly && hiddenEntries > 0 && <div className="setting-row"><div><h2>{t.hiddenInstances}</h2><p>{hiddenEntries}</p></div><button className="button secondary" onClick={restoreHidden} disabled={working || busy}>{t.restoreHidden}</button></div>}
      {!setupOnly && <><div className="setting-row"><div><h2>{t.root}</h2><p>{t.rootPath}</p><code className="settings-path">{root}</code></div><button className="button secondary" onClick={chooseRoot} disabled={working || busy}>{t.changeRoot}</button></div>
      <div className="setting-row"><div><h2>{t.offlineNetwork}</h2><p>{t.offlineNetworkCopy}</p></div><label className="check-row"><input type="checkbox" checked={offlineDraft} onChange={event => setOfflineDraft(event.target.checked)}/>{t.offlineNetwork}</label></div>
      <div className="settings-actions"><span role="status">{offlineDraft === offlineSaved ? '' : t.unsaved}</span><button className="button primary" onClick={saveNetworkOffline} disabled={working || busy || offlineDraft === offlineSaved}>{working ? t.starting : t.saveOffline}</button></div></>}
    </section>
  </>
}

function ConfirmDialog({ t, task, working, cancel, confirm }: { t: ReturnType<typeof getMessages>; task: TaskSnapshot; working: boolean; cancel: () => void; confirm: () => void }) {
  const isLaunch = task.kind === 'launch'
  return <ModalFrame title={isLaunch ? t.stopGameTitle : t.cancelTaskTitle} close={cancel} closeLabel={t.close} role="alertdialog" descriptionId="confirm-copy"><p id="confirm-copy">{isLaunch ? t.stopGameCopy : t.cancelTaskCopy}</p><div className="modal-actions"><button type="button" className="button secondary" onClick={cancel} disabled={working}>{t.keepRunning}</button><button type="button" className="button danger" onClick={confirm} disabled={working}>{working ? t.starting : t.confirmCancel}</button></div></ModalFrame>
}

function PageHeading({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) { return <div className="page-heading"><span className="eyebrow">{eyebrow}</span><h1>{title}</h1><p>{description}</p></div> }
function Landscape() {
  return <svg className="landscape" viewBox="0 0 360 190" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
    <circle className="halo" cx="232" cy="74" r="61"/><circle className="halo" cx="232" cy="74" r="46"/><circle className="moon" cx="232" cy="74" r="29"/><g className="stars" fill="currentColor"><path d="m126 27 2 5 5 2-5 2-2 5-2-5-5-2 5-2zM301 39l2 4 4 2-4 2-2 4-2-4-4-2 4-2z"/><circle cx="157" cy="64" r="1.7"/><circle cx="285" cy="97" r="1.5"/></g>
    <path className="ridge ridge-back" d="M0 177 97 102l60 51 65-39 138 76H0z"/><path className="ridge ridge-front" d="m75 190 97-67 59 37 49-43 80 62v11z"/><path className="cat" d="m203 136 3-17 13 9h13l14-9 2 18c1 10-9 16-22 16s-24-7-23-17Z"/><path className="cat-face" d="m213 138 4 1m21-1-4 1m-11 5 3 2 3-2"/>
  </svg>
}
