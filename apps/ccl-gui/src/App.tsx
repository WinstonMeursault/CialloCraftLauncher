import { InstanceMemorySettings } from './MemorySettings'
import { memo, useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { boundLogLines, boundLogText } from './types'
import type {
  AccountListing, Bootstrap, GameVersion, ImportPreview, ImportRequest, Language,
  LaunchRequest, LibraryEntry, LoaderVersion, Preferences, RunRecord, TaskSnapshot, Theme,
} from './types'
import { acceptTaskSnapshot, formatGuiError, retainBusyEntries, taskIsActive, resolveSelectedInstance } from './types'
import { getMessages } from './i18n'
import { InstanceDetails, Discover, ResultView } from './Advanced'
import { MaintenancePanel } from './Maintenance'
import { AccountsPanel } from './AccountsPanel'
import { ModalFrame } from './Dialog'
import { ProgressDialog } from './ProgressDialog'
import { Home, InstanceIcon } from './Home'
import { Navigation, Icon, type Page } from './Navigation'
import { AppearanceSettings } from './AppearanceSettings'
import defaultWallpaper from './assets/default-wallpaper.webp'
import { LauncherLogs } from './LauncherLogs'

type Modal = 'create' | 'import' | null
type LaunchForm = { accountId: string; java: string; autoInstallJava: boolean; dryRun: boolean }

const LOADERS = ['fabric', 'quilt', 'legacyfabric', 'optifine', 'forge', 'neoforge'] as const
const MemoInstanceDetails = memo(InstanceDetails)
const MemoDiscover = memo(Discover)
const MemoAccountsPanel = memo(AccountsPanel)
const MemoLibraryPage = memo(LibraryPage)
const MemoLogsPage = memo(LogsPage)

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

export default function App() {
  const desktop = useMemo(isDesktopRuntime, [])
  const [page, setPage] = useState<Page>('home')
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null)
  const bootstrapRef = useRef<Bootstrap | null>(null)
  bootstrapRef.current = bootstrap
  const [task, setTask] = useState<TaskSnapshot | null>(null)
  const taskCursor = useRef<TaskSnapshot | null>(null)
  const [launchPending, setLaunchPending] = useState(false)
  const [accountListing, setAccountListing] = useState<AccountListing>({ accounts: [], selected: null })
  const [pageNotice, setPageNotice] = useState<string | null>(null)
  const [background, setBackground] = useState<string | null>(null)
  const [reduceTransparency, setReduceTransparency] = useState(false)
  const [reduceMotion, setReduceMotion] = useState(false)
  const [settingsCategory, setSettingsCategory] = useState('appearance')
  const [detailOpen, setDetailOpen] = useState(false)
  const [dragging, setDragging] = useState(false)
  const libraryScroll = useRef(0)
  const librarySurface = useRef<HTMLDivElement>(null)
  const selectionQueue = useRef<Promise<unknown>>(Promise.resolve())
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
  const selectedKeyRef = useRef(selectedKey)
  selectedKeyRef.current = selectedKey
  const accountReadSequence = useRef(0)
  const accountMutating = useRef(false)
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
  const [launchForm, setLaunchForm] = useState<LaunchForm>({ accountId: '', java: '', autoInstallJava: true, dryRun: false })
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
  const hasUnsavedPreferences = !savedPreferences || theme !== savedPreferences.theme || language !== savedPreferences.language || reduceTransparency !== savedPreferences.reduce_transparency || reduceMotion !== savedPreferences.reduce_motion
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
        setSelectedKey(current => merged.entries.some(entry => entry.key === current) ? current : null)
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
      setSavedPreferences(result.preferences); setReduceTransparency(result.preferences.reduce_transparency); setReduceMotion(result.preferences.reduce_motion)
      setLaunchForm(current => ({ ...current, java: result.launch_defaults.java ?? '', autoInstallJava: result.launch_defaults.auto_install_java }))
      setSelectedKey(result.entries.some(entry => entry.key === result.preferences.last_selected_instance) ? result.preferences.last_selected_instance : null)
      if (result.preferences.background_image) {
        void invoke<string | null>('background_image_data').then(setBackground).catch(() => setBackground(null))
      } else {
        setBackground(null)
      }
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
      const hover=await listen<boolean>('ccl-archive-hover', event => setDragging(event.payload))
      if(disposed){hover();return}stops.push(hover)
      const drop=await listen<string>('ccl-archive-drop', event=>{
        setDragging(false)
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

  const entries = useMemo(() => (bootstrap?.entries ?? []).filter(entry => {
    const displayName = entry.display_name
    const haystack = `${displayName} ${entry.id} ${entry.minecraft} ${entry.loader ?? ''} ${entry.game_directory}`.toLocaleLowerCase()
    return haystack.includes(query.toLocaleLowerCase().trim())
  }), [bootstrap?.entries, query])
  const selected = resolveSelectedInstance(bootstrap?.entries ?? [], selectedKey)

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
      setBootstrap(result); setSelectedKey(null); setDetailOpen(false)
      if (!result.setup_required) await loadAuxiliary()
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const chooseRoot = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Bootstrap | null>('select_root')
      if (result) {
        setBootstrap(result); setSelectedKey(null); setDetailOpen(false)
        if (!result.setup_required) await loadAuxiliary()
      }
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }

  const openCreate = useCallback(async () => {
    setModal('create'); setVersions([]); setVersionId(''); setInstanceId(''); setLoader(''); setLoaderVersions([]); setLoaderVersion(''); setVersionKind('all')
    loaderRequest.current += 1; setLoaderLoading(false)
    setVersionsLoading(true); setError(null)
    try {
      const items = await invoke<GameVersion[]>('list_versions')
      setVersions(items)
      setVersionId(items[0]?.id ?? '')
    } catch (cause) { captureError(cause) }
    finally { setVersionsLoading(false) }
  }, [captureError])

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

  const inspectImport = useCallback(async () => {
    if (working || activeTask) return
    setWorking(true)
    setError(null)
    try {
      const preview = await invoke<ImportPreview | null>('inspect_import')
      if (!preview) return
      setImportPreview(preview); setImportId(importIdFromPath(preview.path)); setAllowedHosts([]); setManualHosts(''); setIncludeOptional(false); setReplaceModified(false)
    } catch (cause) { captureError(cause) }
    finally { setWorking(false) }
  }, [working, activeTask?.id, captureError])

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
    if (accountMutating.current) return
    const sequence = ++accountReadSequence.current
    setError(null)
    try {
      const listing = await invoke<AccountListing>('account_list')
      if (sequence !== accountReadSequence.current || accountMutating.current) return
      setAccountListing(listing)
      setLaunchForm(current => ({ ...current, accountId: listing.selected || '' }))
    }
    catch (cause) { if (sequence === accountReadSequence.current) captureError(cause) }
  }, [captureError])

  const selectEntry = useCallback((key: string) => {
    setSelectedKey(key)
    selectionQueue.current = selectionQueue.current.catch(() => {}).then(() => invoke<Preferences>('select_instance', { key })).then(value => {
      if (value) setSavedPreferences(current => current ? { ...current, last_selected_instance: key } : current)
    }).catch(captureError)
  }, [captureError])

  const enterPage = useCallback((next: Page) => {
    setPage(next); setPageNotice(null)
    if (next === 'accounts' && !bootstrap?.setup_required) void loadAccounts()
    if (next === 'logs' && !bootstrap?.setup_required) void refreshTaskState()
  }, [bootstrap?.setup_required, loadAccounts, refreshTaskState])

  const accountSelect = useCallback(async (id: string) => {
    if (accountMutating.current) return
    accountMutating.current = true; accountReadSequence.current += 1
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_select', { id })
      setAccountListing(listing); setLaunchForm(current => ({ ...current, accountId: id }))
    }
    catch (cause) { captureError(cause) }
    finally { accountMutating.current = false; setWorking(false) }
  }, [captureError])
  const accountLogout = useCallback(async (id: string) => {
    if (accountMutating.current) return
    accountMutating.current = true; accountReadSequence.current += 1
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_logout', { id })
      setAccountListing(listing); setLaunchForm(current => ({ ...current, accountId: current.accountId === id ? listing.selected ?? '' : current.accountId }))
    }
    catch (cause) { captureError(cause) }
    finally { accountMutating.current = false; setWorking(false) }
  }, [captureError])
  const addOfflineAccount = useCallback(async (username: string) => {
    if (accountMutating.current) return
    accountMutating.current = true; accountReadSequence.current += 1
    setWorking(true); setError(null)
    try {
      const listing = await invoke<AccountListing>('account_add_offline', { username })
      setAccountListing(listing)
      setLaunchForm(current => ({ ...current, accountId: listing.selected ?? '' }))
    } catch (cause) { captureError(cause); throw cause }
    finally { accountMutating.current = false; setWorking(false) }
  }, [captureError])
  useEffect(() => {
    if (bootstrap && !bootstrap.setup_required) void loadAccounts()
  }, [bootstrap?.config_path, bootstrap?.setup_required, loadAccounts])

  const savePreferences = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Preferences>('save_preferences', { preferences: { theme, language, reduce_transparency: reduceTransparency, reduce_motion: reduceMotion } })
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

  const launch = async (event: FormEvent) => {
    event.preventDefault()
    if (!selected || working || activeTask) return
    const account = accountListing.accounts.find(item => item.id === launchForm.accountId)
    if (!account) { enterPage('accounts'); return }
    const request: LaunchRequest = {
      key: selected.key,
      identity: account.account_type === 'offline'
        ? { kind: 'offline', username: account.username, account_id: account.id }
        : { kind: 'microsoft', username: null, account_id: account.id },
      java: launchForm.java.trim() || null,
      memory: null,
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
      if (sequence === logReadSequence.current && selectedKeyRef.current === key) setRunLog(boundLogText(logTail(result.text, Math.max(1, Math.min(lines, 1000))), 1000))
    } catch (cause) {
      const payload = asRecord(cause)
      if (sequence === logReadSequence.current && selectedKeyRef.current === key) {
        if (payload?.code === 'log_read' && typeof payload.message === 'string' && /no launch logs found/i.test(payload.message)) setRunLog('')
        else captureError(cause)
      }
    }
    finally { if (sequence === logReadSequence.current && selectedKeyRef.current === key) setLogLoading(false) }
  }, [captureError])

  const loadRuns = useCallback(async (key: string, preferred: string | null = null) => {
    const sequence = ++runListSequence.current
    try {
      const result = await invoke<RunRecord[]>('log_runs', { key })
      if (sequence !== runListSequence.current || selectedKeyRef.current !== key) return
      const bounded = result.slice(0, 100)
      setRuns(bounded)
      const selectedRun = preferred && bounded.some(run => run.run_id === preferred) ? preferred : bounded[0]?.run_id ?? null
      setSelectedRunId(selectedRun)
      if (selectedRun) await readLog(key, selectedRun, 500)
      else setRunLog('')
    } catch (cause) { if (sequence === runListSequence.current && selectedKeyRef.current === key) captureError(cause) }
  }, [captureError, readLog])

  const previousLogKey = useRef<string | null>(selectedKey)
  useEffect(() => {
    if (previousLogKey.current === selectedKey) return
    previousLogKey.current = selectedKey
    logReadSequence.current += 1; runListSequence.current += 1; setLogLoading(false)
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

  const openImport = useCallback(() => { setModal('import'); setImportPreview(null); setImportId(''); setIncludeOptional(false); setReplaceModified(false); setUpdateExisting(false); setAllowedHosts([]); setManualHosts(''); void inspectImport() }, [inspectImport])
  const manageEntry = useCallback((key: string) => { libraryScroll.current = librarySurface.current?.scrollTop ?? 0; selectEntry(key); setDetailOpen(true); enterPage('library') }, [enterPage, selectEntry])
  const receiveTask = useCallback((incoming: TaskSnapshot) => { applyTask(incoming); setProgressVisible(true) }, [applyTask])
  const onLibraryChanged = useCallback(() => { void refreshBootstrap() }, [refreshBootstrap])
  const onSelectEntryToPlay = useCallback((key: string) => { selectEntry(key); enterPage('home') }, [selectEntry, enterPage])
  const onManageSelected = useCallback(() => { if (selected) manageEntry(selected.key) }, [manageEntry, selected])
  const onHomeAccounts = useCallback(() => enterPage('accounts'), [enterPage])
  const onHomeLibrary = useCallback(() => { setDetailOpen(false); enterPage('library') }, [enterPage])
  const onRefreshLibrary = useCallback(() => { void refreshBootstrap() }, [refreshBootstrap])
  const onSelectRun = useCallback((runId: string | null) => {
    setSelectedRunId(runId)
    if (selectedKey) void readLog(selectedKey, runId, 500)
  }, [readLog, selectedKey])
  const onRefreshRuns = useCallback(() => {
    if (selectedKey) void loadRuns(selectedKey, selectedRunId)
  }, [loadRuns, selectedKey, selectedRunId])
  const currentLaunchName = useMemo(() => bootstrap?.entries.find(entry => entry.key === currentLaunchKey)?.display_name ?? null, [bootstrap?.entries, currentLaunchKey])
  const appearance = <AppearanceSettings language={language} theme={theme} setTheme={setTheme} setLanguage={setLanguage} reduceTransparency={reduceTransparency} setReduceTransparency={setReduceTransparency} reduceMotion={reduceMotion} setReduceMotion={setReduceMotion} working={working || !!activeTask} hasUnsaved={hasUnsavedPreferences} save={savePreferences} changeBackground={async (reset) => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Preferences | null>(reset ? 'reset_background_image' : 'select_background_image')
      if (result) { setSavedPreferences(current => current ? { ...current, background_image: result.background_image } : result); setBackground(await invoke<string | null>('background_image_data')) }
    } catch (cause) { captureError(cause) } finally { setWorking(false) }
  }}/>
  useEffect(() => { if (!selected) setDetailOpen(false) }, [selected?.key])
  const initialized = !!bootstrap && !bootstrap.setup_required
  if (!desktop) return <div className="connection-card"><h1>{t.notConnected}</h1><p>{t.notConnectedCopy}</p></div>

  return <div className={`shell${page === 'home' ? ' is-home' : ''}`} data-reduce-transparency={reduceTransparency} data-reduce-motion={reduceMotion}>
    <div className="wallpaper" aria-hidden="true"><img src={background ?? defaultWallpaper} onError={() => setBackground(null)} alt=""/></div>
    <Navigation page={page} language={language} setup={!initialized} onPage={enterPage}/>
    <main className={`main${page === 'home' ? ' home-main' : ''}`}>
      <div className="page-messages">{error && <div className="alert" role="alert"><span>{error}</span><button className="icon-button" aria-label={t.close} onClick={() => setError(null)}>×</button></div>}{pageNotice && <div className="notice" role="status">{pageNotice}<button className="icon-button" aria-label={t.close} onClick={() => setPageNotice(null)}>×</button></div>}</div>
      {loading ? <div className="loading" role="status">{t.loading}</div> : !bootstrap ? <section className="connection-card"><h1>{t.error}</h1><button className="button secondary" onClick={() => void loadBootstrap()}>{t.refreshLogs}</button>{appearance}<MaintenancePanel language={language} busy={working} entry={null} onChanged={() => void loadBootstrap()} onTask={receiveTask}/></section> : !initialized ? page === 'logs' ? <LauncherLogs language={language}/> : page === 'settings' ? <>{appearance}<button className="button secondary" onClick={chooseRoot} disabled={working}>{t.changeRoot}</button></> : <SetupPage t={t} bootstrap={bootstrap} working={working} chooseRoot={chooseRoot} initializeRoot={initializeRoot}/> : <>
        <div hidden={page !== 'home'} className="page-surface home-surface"><Home language={language} entries={bootstrap.entries} selected={selected} accountListing={accountListing} accountId={launchForm.accountId} disabled={working || !!activeTask} task={task} launchPending={launchPending} missingSelection={!!savedPreferences?.last_selected_instance && !selected} onSelect={selectEntry} onAccount={accountSelect} onAccounts={onHomeAccounts} onManage={onManageSelected} onCreate={openCreate} onImport={openImport} onLibrary={onHomeLibrary} onLaunch={launch} onProgress={() => setProgressVisible(true)}/></div>
        <div ref={librarySurface} hidden={page !== 'library'} className="page-surface" data-page="library"><div hidden={detailOpen}><MemoLibraryPage t={t} language={language} entries={entries} query={query} setQuery={setQuery} selectedKey={selectedKey} onManage={manageEntry} onRefresh={onRefreshLibrary} onPlay={onSelectEntryToPlay} openCreate={openCreate} openImport={openImport} disabled={working || !!activeTask}/></div>{detailOpen && <><button className="button back-button" onClick={() => { setDetailOpen(false); requestAnimationFrame(() => { if (librarySurface.current) librarySurface.current.scrollTop = libraryScroll.current }) }}><Icon name="back"/>{t.library}</button><MemoInstanceDetails language={language} entry={selected} busy={working || !!activeTask} onTask={receiveTask} onChanged={onLibraryChanged}/><details className="launch-settings"><summary>{t.launchOptions}</summary>{selected && <InstanceMemorySettings key={selected.key} entry={selected} language={language} disabled={working || !!activeTask} onSaved={onLibraryChanged}/>}<LaunchOptions t={t} form={launchForm} onChange={setLaunchForm}/><p className="muted-copy">{language === 'zh-CN' ? 'Java 等选项用于本次启动器会话；实例内存设置需单独保存。全局默认值可在设置中修改。' : 'Java and other options apply to this launcher session; save instance memory separately. Change global defaults in Settings.'}</p><button className="button primary" disabled={!selected} onClick={() => enterPage('home')}>{language === 'zh-CN' ? '返回启动' : 'Back to play'}</button></details></>}{bootstrap.warnings.length > 0 && <details className="warnings"><summary>{t.warnings}</summary>{bootstrap.warnings.map((warning, index) => <p key={index}>{warning}</p>)}</details>}</div>
        <div hidden={page !== 'discover'} className="page-surface" data-page="discover"><MemoDiscover entries={bootstrap.entries} onSelectEntry={selectEntry} language={language} entry={selected} busy={working || !!activeTask} onTask={receiveTask} onChanged={onLibraryChanged}/></div>
        <div hidden={page !== 'accounts'} className="page-surface" data-page="accounts"><MemoAccountsPanel language={language} accounts={accountListing.accounts} selected={accountListing.selected} disabled={working || !!activeTask} onAddOffline={addOfflineAccount} onSelect={accountSelect} onRemove={accountLogout}/></div>
        <div hidden={page !== 'logs'} className="page-surface" data-page="logs"><h1>{t.logs}</h1><div className="segmented log-source-switch"><button className={logSource === 'game' ? 'selected' : ''} onClick={() => setLogSource('game')}>{language === 'zh-CN' ? '游戏日志' : 'Game logs'}</button><button className={logSource === 'launcher' ? 'selected' : ''} onClick={() => setLogSource('launcher')}>{language === 'zh-CN' ? 'CCL 日志' : 'CCL logs'}</button></div>{logSource === 'launcher' ? <LauncherLogs language={language} active={page === 'logs'}/> : <MemoLogsPage t={t} entries={bootstrap.entries} selectedKey={selectedKey} setSelectedKey={selectEntry} runs={runs} selectedRunId={selectedRunId} selectRun={onSelectRun} runLog={runLog} logLoading={logLoading} refresh={onRefreshRuns} liveTask={activeLaunch} logsPaused={pauseSavedLogs} liveEntry={currentLaunchName}/>}</div>
        <div hidden={page !== 'settings'} className="page-surface" data-page="settings"><h1>{t.settings}</h1><nav className="settings-tabs" aria-label={language === 'zh-CN' ? '设置类别' : 'Settings categories'}>{[['appearance','外观','Appearance'],['game','游戏与 Java','Game & Java'],['downloads','下载与网络','Downloads & network'],['updates','更新','Updates'],['advanced','高级与诊断','Advanced & diagnostics'],['about','关于','About']].map(([key,cn,en]) => <button key={key} aria-current={settingsCategory === key ? 'page' : undefined} className={settingsCategory === key ? 'active' : ''} onClick={() => setSettingsCategory(key)}>{language === 'zh-CN' ? cn : en}</button>)}</nav>
          <div hidden={settingsCategory !== 'appearance'}>{appearance}</div>
          {settingsCategory === 'game' && <section className="settings-card"><h2>{t.root}</h2><div className="setting-row"><code className="settings-path">{bootstrap.root}</code><button className="button secondary" onClick={chooseRoot} disabled={working || !!activeTask}>{t.changeRoot}</button></div>{bootstrap.hidden_entries > 0 && <div className="setting-row"><span>{t.hiddenInstances} · {bootstrap.hidden_entries}</span><button className="button secondary" disabled={working || !!activeTask} onClick={restoreHidden}>{t.restoreHidden}</button></div>}</section>}
          {page === 'settings' && settingsCategory !== 'appearance' && <MaintenancePanel category={settingsCategory} language={language} busy={working || !!activeTask} entry={selected} onChanged={onLibraryChanged} onTask={receiveTask}/>}
        </div>
      </>}
    </main>

    {dragging && <div className="drop-overlay" role="status"><Icon name="import"/><strong>{language === 'zh-CN' ? '松手导入整合包' : 'Drop to import modpack'}</strong></div>}
    {task && !modal && !pendingCancel && <ProgressDialog task={task} language={language} visible={progressVisible} onVisibility={setProgressVisible} onCancel={() => setPendingCancel(task)} onLogs={() => { setPage('logs'); setLogSource('game'); setProgressVisible(false) }} />}
    {modal === 'create' && <CreateModal error={error} t={t} versions={versions} versionsLoading={versionsLoading} versionId={versionId} setVersionId={chooseVersion} versionKind={versionKind} setVersionKind={setVersionKind} instanceId={instanceId} setInstanceId={setInstanceId} loader={loader} changeLoader={changeLoader} loaderLoading={loaderLoading} loaderVersions={loaderVersions} loaderVersion={loaderVersion} setLoaderVersion={setLoaderVersion} working={working || !!activeTask} close={() => setModal(null)} submit={installInstance} />}
    {modal === 'import' && <ImportModal error={error} language={language} updateExisting={updateExisting} setUpdateExisting={setUpdateExisting} t={t} preview={importPreview} inspect={inspectImport} id={importId} setId={setImportId} includeOptional={includeOptional} setIncludeOptional={setIncludeOptional} replaceModified={replaceModified} setReplaceModified={setReplaceModified} allowedHosts={allowedHosts} setAllowedHosts={setAllowedHosts} manualHosts={manualHosts} setManualHosts={setManualHosts} working={working || !!activeTask} close={() => setModal(null)} submit={importInstance} />}
    {pendingCancel && <ConfirmDialog t={t} task={pendingCancel} working={working} cancel={() => setPendingCancel(null)} confirm={confirmCancel} />}
  </div>
}

function SetupPage({ t, bootstrap, working, chooseRoot, initializeRoot }: { t: ReturnType<typeof getMessages>; bootstrap: Bootstrap; working: boolean; chooseRoot: () => void; initializeRoot: () => void }) {
  return <section className="setup-card">
    <span className="eyebrow">{t.root}</span><h1>{t.setupTitle}</h1><p>{t.setupCopy}</p>
    <div className="setup-path"><span className="root-info-icon"><Icon name="folder"/></span><div><strong>{bootstrap.root_exists ? t.setupFolderExists : t.setupFolderMissing}</strong><code>{bootstrap.root}</code></div></div>
    <div className="setup-actions"><button className="button secondary" onClick={chooseRoot} disabled={working}><Icon name="folder"/>{t.setupChoose}</button><button className="button primary" onClick={initializeRoot} disabled={working}>{working ? t.initializing : bootstrap.root_exists ? t.setupExisting : t.setupNew}</button></div>
  </section>
}

function LibraryPage({ t, language, entries, query, setQuery, selectedKey, onManage, onPlay, onRefresh, openCreate, openImport, disabled }: {
  t: ReturnType<typeof getMessages>; language: Language; entries: LibraryEntry[]; query: string; setQuery: (value: string) => void; selectedKey: string | null; onManage: (key: string) => void; onPlay: (key: string) => void; onRefresh: () => void; openCreate: () => void; openImport: () => void; disabled: boolean
}) {
  const zh = language === 'zh-CN'
  return <section className="library-page"><header className="page-title-row"><h1>{t.library}</h1><div className="heading-actions"><button className="icon-button" disabled={disabled} aria-label={t.refreshLogs} title={t.refreshLogs} onClick={onRefresh}><Icon name="refresh"/></button><button className="button secondary" disabled={disabled} onClick={openImport}><Icon name="import"/>{zh ? '导入' : 'Import'}</button><button className="button primary" disabled={disabled} onClick={openCreate}><Icon name="plus"/>{zh ? '创建实例' : 'Create instance'}</button></div></header>
    <label className="search-field library-search"><Icon name="search"/><input aria-label={t.search} placeholder={t.search} value={query} onChange={event => setQuery(event.target.value)}/></label>
    <div className="library-rows">{entries.map(entry => <article className={`library-row${selectedKey === entry.key ? ' selected' : ''}`} key={entry.key}><button className="instance-row" onClick={() => onPlay(entry.key)} disabled={disabled}><InstanceIcon entry={entry}/><span className="instance-row-copy"><strong>{entry.display_name}</strong><small>{entry.minecraft} · {entry.loader ?? t.vanillaSoftware}</small></span></button><button className="icon-button row-more" aria-label={`${zh ? '管理' : 'Manage'} ${entry.display_name}`} title={zh ? '管理实例' : 'Manage instance'} onClick={() => onManage(entry.key)}><Icon name="more"/></button></article>)}</div>
    {!entries.length && <div className="empty-state"><Icon name="library"/><h2>{t.empty}</h2><p>{t.emptyHint}</p></div>}
  </section>
}
function Meta({ label, value }: { label: string; value: string }) { return <div><dt>{label}</dt><dd>{value}</dd></div> }

function CreateModal({ error, t, versions, versionsLoading, versionId, setVersionId, versionKind, setVersionKind, instanceId, setInstanceId, loader, changeLoader, loaderLoading, loaderVersions, loaderVersion, setLoaderVersion, working, close, submit }: {
  error: string | null; t: ReturnType<typeof getMessages>; versions: GameVersion[]; versionsLoading: boolean; versionId: string; setVersionId: (value: string) => void; versionKind: string; setVersionKind: (value: string) => void; instanceId: string; setInstanceId: (value: string) => void;
  loader: string; changeLoader: (value: string, minecraft?: string) => void; loaderLoading: boolean; loaderVersions: LoaderVersion[]; loaderVersion: string; setLoaderVersion: (value: string) => void; working: boolean; close: () => void; submit: (event: FormEvent) => void
}) {
  const kinds = [...new Set(versions.map(version => version.kind))]
  const visibleVersions = versionKind === 'all' ? versions : versions.filter(version => version.kind === versionKind)
  return <ModalFrame closeLabel={t.close} title={t.newInstance} close={close}>
    <form className="modal-form" onSubmit={submit}>{error && <div className="alert" role="alert">{error}</div>}
      <label className="field"><span>{t.name}</span><input required value={instanceId} onChange={event => setInstanceId(event.target.value)} autoComplete="off"/></label>
      <label className="field"><span>{t.version}</span><select required value={versionId} disabled={versionsLoading || !versions.length} onChange={event => setVersionId(event.target.value)}><option value="">{versionsLoading ? t.loadingVersions : t.noVersions}</option>{visibleVersions.map(item => <option key={item.id} value={item.id}>{item.id} · {item.kind}</option>)}</select></label>
      <label className="field"><span>{t.versionKind}</span><select value={versionKind} onChange={event => { const kind = event.target.value; setVersionKind(kind); if (kind !== 'all' && !versions.some(item => item.id === versionId && item.kind === kind)) setVersionId(versions.find(item => item.kind === kind)?.id ?? '') }}><option value="all">{t.all}</option>{kinds.map(kind => <option key={kind} value={kind}>{kind}</option>)}</select></label>
      <label className="field"><span>{t.softwareType}</span><select value={loader} onChange={event => changeLoader(event.target.value)}><option value="">Vanilla</option>{LOADERS.map(item => <option key={item} value={item}>{item}</option>)}</select></label>
      {loader && <label className="field"><span>{t.loaderVersion}</span><select value={loaderVersion} disabled={loaderLoading || !loaderVersions.length} onChange={event => setLoaderVersion(event.target.value)}><option value="">{loaderLoading ? t.loadingVersions : t.noVersions}</option>{loaderVersions.map(item => <option key={item.version} value={item.version}>{item.version}{item.stable ? '' : ' · preview'}</option>)}</select></label>}
      <div className="modal-actions"><button type="button" className="button secondary" onClick={close}>{t.close}</button><button className="button primary" disabled={working || versionsLoading || !versionId || (!!loader && (!loaderVersion || loaderLoading))}>{working ? t.starting : t.install}</button></div>
    </form>
  </ModalFrame>
}

function ImportModal({ error, language, updateExisting, setUpdateExisting, t, preview, inspect, id, setId, includeOptional, setIncludeOptional, replaceModified, setReplaceModified, allowedHosts, setAllowedHosts, manualHosts, setManualHosts, working, close, submit }: {
  error: string | null; language: Language; updateExisting: boolean; setUpdateExisting:(value:boolean)=>void; t: ReturnType<typeof getMessages>; preview: ImportPreview | null; inspect: () => void; id: string; setId: (value: string) => void; includeOptional: boolean; setIncludeOptional: (value: boolean) => void; replaceModified: boolean; setReplaceModified: (value: boolean) => void; allowedHosts: string[]; setAllowedHosts: (value: string[]) => void; manualHosts: string; setManualHosts: (value: string) => void; working: boolean; close: () => void; submit: (event: FormEvent) => void
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
    <form className="modal-form" onSubmit={submit}>{error && <div className="alert" role="alert">{error}</div>}
      {!preview ? <div className="picker-state"><p>{working ? t.loading : t.inspectPack}</p><button type="button" className="button secondary" disabled={working} onClick={inspect}>{t.inspectPack}</button></div> : <>
        <div className="path-box"><span>{t.archive}</span><code title={preview.path}>{preview.path}</code></div>
        <label className="field"><span>{t.name}</span><input required value={id} onChange={event => setId(event.target.value)}/></label>
        <section className="summary-box"><strong>{t.packSummary}</strong>{shortSummary.map(item => <Meta key={item.key} label={({ format: language === 'zh-CN' ? '格式' : 'Format', name: t.name, version: t.version, minecraft: t.gameVersion, loader: t.softwareType } as Record<string, string>)[item.key] ?? item.key} value={item.value}/>)}<details><summary>{t.rawSummary}</summary><ResultView value={preview?.summary} language={language}/></details></section>
        <label className="check-row"><input type="checkbox" checked={includeOptional} onChange={event => setIncludeOptional(event.target.checked)}/>{t.includeOptional}</label>
        <label className="check-row"><input type="checkbox" checked={updateExisting} onChange={event => setUpdateExisting(event.target.checked)}/>{language==='zh-CN'?'更新已有实例（默认新建；更新前会确认）':'Update an existing instance (creates new by default; confirmation required)'}</label>
        <label className="check-row"><input type="checkbox" checked={replaceModified} onChange={event => setReplaceModified(event.target.checked)}/>{t.replaceModified}</label>
        {hosts.length > 0 && <fieldset className="host-list"><legend>{t.allowedHosts}</legend>{[...new Set(hosts)].map(host => <label className="check-row" key={host}><input type="checkbox" checked={allowedHosts.includes(host)} onChange={event => setAllowedHosts(event.target.checked ? [...allowedHosts, host] : allowedHosts.filter(item => item !== host))}/>{host}</label>)}</fieldset>}
        <label className="field"><span>{t.manualHosts}</span><textarea rows={2} value={manualHosts} onChange={event => setManualHosts(event.target.value)} placeholder="cdn.example.org"/></label>
        <button type="button" className="text-button" onClick={inspect}>{t.inspectAgain}</button>
      </>}
      <div className="modal-actions"><button type="button" className="button secondary" onClick={close}>{t.close}</button><button className="button primary" disabled={working || !preview || !id.trim()}>{working ? t.starting : t.import}</button></div>
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

function ConfirmDialog({ t, task, working, cancel, confirm }: { t: ReturnType<typeof getMessages>; task: TaskSnapshot; working: boolean; cancel: () => void; confirm: () => void }) {
  const isLaunch = task.kind === 'launch' && task.pid !== null
  return <ModalFrame title={isLaunch ? t.stopGameTitle : t.cancelTaskTitle} close={cancel} closeLabel={t.close} role="alertdialog" descriptionId="confirm-copy"><p id="confirm-copy">{isLaunch ? t.stopGameCopy : t.cancelTaskCopy}</p><div className="modal-actions"><button type="button" className="button secondary" onClick={cancel} disabled={working}>{t.keepRunning}</button><button type="button" className="button danger" onClick={confirm} disabled={working}>{working ? t.starting : t.confirmCancel}</button></div></ModalFrame>
}

function PageHeading({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) { return <div className="page-heading"><span className="eyebrow">{eyebrow}</span><h1>{title}</h1><p>{description}</p></div> }

function LaunchOptions({ t, form, onChange }: { t: ReturnType<typeof getMessages>; form: LaunchForm; onChange: (value: LaunchForm) => void }) {
  return <div className="launch-options-body"><label className="field"><span>{t.javaPath}</span><input value={form.java} onChange={event => onChange({ ...form, java: event.target.value })}/></label><label className="check-row"><input type="checkbox" checked={form.autoInstallJava} onChange={event => onChange({ ...form, autoInstallJava: event.target.checked })}/>{t.autoInstallJava}</label><label className="check-row"><input type="checkbox" checked={form.dryRun} onChange={event => onChange({ ...form, dryRun: event.target.checked })}/>{t.dryRun}</label></div>
}
