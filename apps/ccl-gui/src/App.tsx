import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Bootstrap, Language, LibraryEntry, Preferences, TaskSnapshot, Theme } from './types'
import { acceptTaskSnapshot } from './types'
import { getMessages } from './i18n'

type Page = 'library' | 'tasks' | 'settings'
type Filter = 'all' | 'managed' | 'vanilla'
type GuiErrorPayload = { code?: unknown; message?: unknown }

function formatGuiError(cause: unknown, fallback: string): string {
  if (cause && typeof cause === 'object') {
    const payload = cause as GuiErrorPayload
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

function isDesktopRuntime() {
  return '__TAURI_INTERNALS__' in window
}

function Icon({ name }: { name: 'library' | 'compass' | 'user' | 'tasks' | 'logs' | 'settings' | 'folder' | 'search' }) {
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
  const [task, setTask] = useState<TaskSnapshot | null>(null)
  const taskCursor = useRef<TaskSnapshot | null>(null)
  const [theme, setTheme] = useState<Theme>('system')
  const [language, setLanguage] = useState<Language>('en')
  const languageRef = useRef(language)
  languageRef.current = language
  const [savedPreferences, setSavedPreferences] = useState<Preferences | null>(null)
  const [loading, setLoading] = useState(true)
  const [working, setWorking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [filter, setFilter] = useState<Filter>('all')
  const [query, setQuery] = useState('')
  const [selectedKey, setSelectedKey] = useState<string | null>(null)

  const t = getMessages(language)
  const hasUnsavedPreferences = !!savedPreferences && (theme !== savedPreferences.theme || language !== savedPreferences.language)

  const applyTask = useCallback((incoming: TaskSnapshot) => {
    const accepted = acceptTaskSnapshot(taskCursor.current, incoming)
    if (accepted === taskCursor.current) return
    taskCursor.current = accepted
    setTask(accepted)
  }, [])

  const load = useCallback(async () => {
    if (!desktop) { setLoading(false); return }
    setLoading(true)
    setError(null)
    try {
      const result = await invoke<Bootstrap>('bootstrap')
      setBootstrap(result)
      setTheme(result.preferences.theme)
      setLanguage(result.preferences.language)
      setSavedPreferences(result.preferences)
      setSelectedKey(current => result.entries.some(entry => entry.key === current) ? current : result.entries[0]?.key ?? null)
      const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
      if (snapshot) applyTask(snapshot)
      else {
        taskCursor.current = null
        setTask(null)
      }
    } catch (cause) {
      setError(formatGuiError(cause, getMessages(languageRef.current).unknownError))
    } finally {
      setLoading(false)
    }
  }, [desktop, applyTask])

  useEffect(() => {
    if (!desktop) { setLoading(false); return }
    let disposed = false
    let unlisten: (() => void) | undefined
    void (async () => {
      try {
        const stopListening = await listen<TaskSnapshot>('gui-task', event => {
          if (!disposed) applyTask(event.payload)
        })
        if (disposed) {
          stopListening()
          return
        }
        unlisten = stopListening
        await load()
      } catch (cause) {
        if (!disposed) {
          setError(formatGuiError(cause, getMessages(languageRef.current).unknownError))
          setLoading(false)
        }
      }
    })()
    const onVisibility = () => {
      void invoke('set_visibility', { visible: !document.hidden }).then(async () => {
        if (!document.hidden) {
          const snapshot = await invoke<TaskSnapshot | null>('task_snapshot')
          if (!disposed) {
            if (snapshot) applyTask(snapshot)
            else {
              taskCursor.current = null
              setTask(null)
            }
          }
        }
      }).catch(cause => {
        if (!disposed) setError(formatGuiError(cause, getMessages(languageRef.current).unknownError))
      })
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      disposed = true
      unlisten?.()
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [desktop, load, applyTask])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const update = () => document.documentElement.dataset.theme = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [theme])

  const entries = (bootstrap?.entries ?? []).filter(entry => {
    const matchesFilter = filter === 'all' || (filter === 'managed' ? entry.kind === 'managed' : entry.kind === 'vanilla')
    const haystack = `${entry.id} ${entry.minecraft} ${entry.loader ?? ''} ${entry.game_directory}`.toLocaleLowerCase()
    return matchesFilter && haystack.includes(query.toLocaleLowerCase().trim())
  })

  const selected = entries.find(entry => entry.key === selectedKey) ?? entries[0] ?? null

  const chooseRoot = async () => {
    setWorking(true); setError(null)
    try {
      const result = await invoke<Bootstrap | null>('select_root')
      if (result) {
        setBootstrap(result)
        setSelectedKey(result.entries[0]?.key ?? null)
      }
    } catch (cause) { setError(formatGuiError(cause, t.unknownError)) }
    finally { setWorking(false) }
  }

  const savePreferences = async () => {
    setWorking(true); setError(null)
    const preferences = { theme, language }
    try {
      const result = await invoke<Preferences>('save_preferences', { preferences })
      setTheme(result.theme); setLanguage(result.language); setSavedPreferences(result)
    } catch (cause) { setError(formatGuiError(cause, t.unknownError)) }
    finally { setWorking(false) }
  }

  const startProbe = async () => {
    setWorking(true); setError(null)
    try { applyTask(await invoke<TaskSnapshot>('start_probe')) }
    catch (cause) { setError(formatGuiError(cause, t.unknownError)) }
    finally { setWorking(false) }
  }

  const cancelTask = async () => {
    if (!task) return
    setWorking(true); setError(null)
    try { applyTask(await invoke<TaskSnapshot>('cancel_task', { id: task.id })) }
    catch (cause) { setError(formatGuiError(cause, t.unknownError)) }
    finally { setWorking(false) }
  }

  const openTasks = () => setPage('tasks')
  return <div className="shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark" aria-hidden="true">✧</div><div><strong>CCL</strong><small>{t.brand}</small></div></div>
      <nav aria-label={t.brand}>
        <NavItem icon="library" label={t.library} active={page === 'library'} onClick={() => setPage('library')} />
        <NavItem icon="compass" label={t.discover} disabled unavailableLabel={t.unavailable} />
        <NavItem icon="user" label={t.accounts} disabled unavailableLabel={t.unavailable} />
        <NavItem icon="tasks" label={t.tasks} active={page === 'tasks'} onClick={openTasks} />
        <NavItem icon="logs" label={t.logs} disabled unavailableLabel={t.unavailable} />
      </nav>
      <div className="sidebar-bottom">
        <NavItem icon="settings" label={t.settings} active={page === 'settings'} onClick={() => setPage('settings')} />
        <p>{t.tagline}</p>
      </div>
    </aside>

    <main className="main">
      <header className="topline">
        <span className="crumb">{t.brand} <span aria-hidden="true">/</span> {page === 'library' ? t.library : page === 'tasks' ? t.tasks : t.settings}</span>
        <div className="top-actions"><span className="prototype-badge">{t.prototype}</span><button className="locale-switch" onClick={() => setLanguage(language === 'en' ? 'zh-CN' : 'en')}>{language === 'en' ? '简体中文' : 'English'}</button></div>
      </header>
      {error && <div className="alert" role="alert"><strong>{t.error}</strong><span>{error}</span><button className="icon-button" aria-label="Dismiss" onClick={() => setError(null)}>×</button></div>}
      {!desktop ? <section className="connection-card"><div className="connection-symbol">!</div><h1>{t.notConnected}</h1><p>{t.notConnectedCopy}</p></section> : loading ? <div className="loading" role="status">{t.loading}</div> : <>
        {page === 'library' && <LibraryPage t={t} entries={entries} selected={selected} bootstrap={bootstrap} filter={filter} query={query} setFilter={setFilter} setQuery={setQuery} setSelectedKey={setSelectedKey} chooseRoot={chooseRoot} working={working} onTasks={openTasks} />}
        {page === 'tasks' && <TasksPage t={t} task={task} working={working} startProbe={startProbe} cancelTask={cancelTask} />}
        {page === 'settings' && <SettingsPage t={t} theme={theme} language={language} setTheme={setTheme} setLanguage={setLanguage} save={savePreferences} working={working} hasUnsaved={hasUnsavedPreferences} saved={!!savedPreferences && !hasUnsavedPreferences} />}
      </>}
    </main>
  </div>
}

function NavItem({ icon, label, active = false, disabled = false, unavailableLabel, onClick }: { icon: 'library' | 'compass' | 'user' | 'tasks' | 'logs' | 'settings'; label: string; active?: boolean; disabled?: boolean; unavailableLabel?: string; onClick?: () => void }) {
  return <button className={`nav-item${active ? ' active' : ''}${disabled ? ' disabled' : ''}`} onClick={onClick} disabled={disabled} title={disabled ? `${label} · Not yet available` : label} aria-current={active ? 'page' : undefined}>
    <Icon name={icon} /><span>{label}</span>{disabled && <span className="nav-disabled">{unavailableLabel}</span>}{active && <span className="active-dot" aria-hidden="true" />}
  </button>
}

function LibraryPage({ t, entries, selected, bootstrap, filter, query, setFilter, setQuery, setSelectedKey, chooseRoot, working, onTasks }: {
  t: ReturnType<typeof getMessages>; entries: LibraryEntry[]; selected: LibraryEntry | null; bootstrap: Bootstrap | null; filter: Filter; query: string;
  setFilter: (value: Filter) => void; setQuery: (value: string) => void; setSelectedKey: (value: string) => void; chooseRoot: () => void; working: boolean; onTasks: () => void
}) {
  return <>
    <section className="hero"><div className="hero-copy"><div className="eyebrow">{t.welcome}</div><h1>{t.heading}</h1><p>{t.subheading}</p></div><Landscape /></section>
    {!bootstrap?.root_exists && <div className="root-notice"><Icon name="folder"/><span>{t.rootMissing}</span></div>}
    <div className="section-heading"><div><h2>{t.instances}</h2><p>{bootstrap?.entries.length ?? 0} · {bootstrap?.root_exists ? t.rootExists : t.rootDoesNotExist}</p></div><div className="heading-actions"><button className="button secondary" onClick={onTasks}><Icon name="tasks" />{t.tasks}</button><button className="button secondary" onClick={chooseRoot} disabled={working}><Icon name="folder" />{working ? t.starting : bootstrap?.root ? t.changeRoot : t.chooseRoot}</button></div></div>
    <div className="library-layout">
      <section className="library-list" aria-label={t.instances}>
        <label className="search-field"><Icon name="search"/><input value={query} onChange={event => setQuery(event.target.value)} placeholder={t.search} /></label>
        <div className="filters" role="group" aria-label={t.instances}>{(['all', 'managed', 'vanilla'] as const).map(item => <button key={item} className={filter === item ? 'selected' : ''} aria-pressed={filter === item} onClick={() => setFilter(item)}>{item === 'all' ? t.all : item === 'managed' ? t.managed : t.vanilla}</button>)}</div>
        <div className="entry-list">{entries.map((entry, index) => <EntryCard key={entry.key} entry={entry} selected={entry.key === selected?.key} index={index} t={t} onClick={() => setSelectedKey(entry.key)} />)}
          {entries.length === 0 && <div className="empty-state"><div>⌕</div><strong>{t.empty}</strong><p>{t.emptyHint}</p></div>}
        </div>
        <div className="root-info"><span className="root-info-icon"><Icon name="folder"/></span><div><strong>{t.root}</strong><span title={bootstrap?.root ?? ''}>{bootstrap?.root ?? '—'}</span></div></div>
      </section>
      <section className="detail-card" aria-label={t.selected}>
        {selected ? <>
          <div className="detail-head"><span>{t.selected}</span><span className="badge">{selected.kind === 'managed' ? t.managed : t.vanilla}</span></div>
          <h2>{selected.id}</h2><p className="detail-subtitle">Minecraft {selected.minecraft}</p>
          <dl className="metadata">
            <Meta label={t.id} value={selected.id}/><Meta label={t.gameVersion} value={selected.minecraft}/><Meta label={t.loader} value={selected.loader ?? 'Vanilla'}/><Meta label={t.kind} value={selected.kind === 'managed' ? t.managed : t.vanilla}/>
          </dl>
          <div className="path-box"><span>{t.gameDirectory}</span><code title={selected.game_directory}>{selected.game_directory}</code></div>
          <button className="button primary launch-button" disabled title={t.launchHint}>▷ &nbsp; {t.launch}</button><p className="disabled-hint">{t.launchHint}</p>
        </> : <div className="empty-detail"><div className="empty-detail-mark">✧</div><h2>{t.empty}</h2><p>{t.emptyHint}</p></div>}
      </section>
    </div>
    <div className="root-footnote"><span>{t.rootPath}: <code>{bootstrap?.root ?? '—'}</code></span><span>{t.configPath}: <code>{bootstrap?.config_path ?? '—'}</code></span></div>
    {(bootstrap?.warnings.length ?? 0) > 0 && <section className="warnings"><strong>{t.warnings}</strong>{bootstrap?.warnings.map((warning, i) => <p key={`${i}-${warning}`}>{warning}</p>)}</section>}
  </>
}

function EntryCard({ entry, selected, index, t, onClick }: { entry: LibraryEntry; selected: boolean; index: number; t: ReturnType<typeof getMessages>; onClick: () => void }) {
  return <button className={`entry-card${selected ? ' selected' : ''}`} onClick={onClick} aria-pressed={selected}>
    <span className={`entry-tile tile-${index % 3}`} aria-hidden="true">{entry.kind === 'vanilla' ? '▧' : entry.loader ? '✧' : '◈'}</span>
    <span className="entry-copy"><strong>{entry.id}</strong><span>{entry.kind === 'managed' ? t.managed : t.vanilla}</span><span className="tags"><i>{entry.minecraft}</i><i>{entry.loader ?? 'Vanilla'}</i></span></span>
    {selected && <span className="selected-check" aria-label="Selected">✓</span>}
  </button>
}

function Meta({ label, value }: { label: string; value: string }) { return <div><dt>{label}</dt><dd>{value}</dd></div> }

function TasksPage({ t, task, working, startProbe, cancelTask }: { t: ReturnType<typeof getMessages>; task: TaskSnapshot | null; working: boolean; startProbe: () => void; cancelTask: () => void }) {
  const active = task?.state === 'running' || task?.state === 'cancelling'
  const percent = task && task.total > 0 ? Math.min(100, task.completed / task.total * 100) : 0
  const logs = task?.logs.slice(-100) ?? []
  return <>
    <PageHeading eyebrow={t.tasks} title={t.taskIntro} description={t.probeDescription} />
    <section className="task-panel">
      <div className="task-panel-head"><div className="task-glyph"><Icon name="tasks"/></div><div className="task-title"><strong>{task ? t.taskSynthetic : t.noTask}</strong><span>{task ? `#${task.id} · ${t.state[task.state]}` : t.noTaskCopy}</span></div>
        {active ? <button className="button danger" onClick={cancelTask} disabled={working || task.state === 'cancelling'}>{task.state === 'cancelling' ? t.state.cancelling : t.cancel}</button> : <button className="button primary" onClick={startProbe} disabled={working}>{working ? t.starting : t.startProbe}</button>}
      </div>
      {task && <div className="task-progress"><div className="progress-label"><span>{t.progress}</span><span>{task.completed.toLocaleString()} / {task.total.toLocaleString()}</span></div><div className="progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={task.total} aria-valuenow={task.completed}><span style={{ width: `${percent}%` }}/></div>{task.error && <p className="task-error">{task.error}</p>}</div>}
      <div className="log-wrap"><div className="log-heading"><strong>{t.logHeading}</strong><span>{logs.length} / 100</span></div><pre className="log-view" aria-label={t.logHeading}>{logs.length ? logs.join('\n') : t.noLogs}</pre></div>
    </section>
  </>
}

function SettingsPage({ t, theme, language, setTheme, setLanguage, save, working, hasUnsaved, saved }: { t: ReturnType<typeof getMessages>; theme: Theme; language: Language; setTheme: (theme: Theme) => void; setLanguage: (language: Language) => void; save: () => void; working: boolean; hasUnsaved: boolean; saved: boolean }) {
  return <>
    <PageHeading eyebrow={t.settings} title={t.appearance} description={t.appearanceCopy} />
    <section className="settings-card">
      <div className="setting-row"><div><h2>{t.theme}</h2><p>{t.appearanceCopy}</p></div><div className="segmented" role="group" aria-label={t.theme}>{(['system', 'light', 'dark'] as const).map(value => <button key={value} aria-pressed={theme === value} className={theme === value ? 'selected' : ''} onClick={() => setTheme(value)}>{value === 'system' ? t.system : value === 'light' ? t.light : t.dark}</button>)}</div></div>
      <div className="setting-row"><div><h2>{t.language}</h2><p>{t.language}</p></div><div className="segmented" role="group" aria-label={t.language}><button className={language === 'en' ? 'selected' : ''} aria-pressed={language === 'en'} onClick={() => setLanguage('en')}>{t.english}</button><button className={language === 'zh-CN' ? 'selected' : ''} aria-pressed={language === 'zh-CN'} onClick={() => setLanguage('zh-CN')}>{t.chinese}</button></div></div>
      <div className="settings-actions"><span role="status">{hasUnsaved ? t.unsaved : saved ? t.saved : ''}</span><button className="button primary" onClick={save} disabled={working || !hasUnsaved}>{working ? t.starting : t.save}</button></div>
    </section>
  </>
}

function PageHeading({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return <div className="page-heading"><span className="eyebrow">{eyebrow}</span><h1>{title}</h1><p>{description}</p></div>
}

function Landscape() {
  return <svg className="landscape" viewBox="0 0 360 190" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
    <circle className="halo" cx="232" cy="74" r="61"/><circle className="halo" cx="232" cy="74" r="46"/><circle className="moon" cx="232" cy="74" r="29"/>
    <g className="stars" fill="currentColor"><path d="m126 27 2 5 5 2-5 2-2 5-2-5-5-2 5-2zM301 39l2 4 4 2-4 2-2 4-2-4-4-2 4-2z"/><circle cx="157" cy="64" r="1.7"/><circle cx="285" cy="97" r="1.5"/><circle cx="330" cy="20" r="1.5"/></g>
    <path className="ridge ridge-back" d="M0 177 97 102l60 51 65-39 138 76H0z"/><path className="ridge ridge-front" d="m75 190 97-67 59 37 49-43 80 62v11z"/>
    <path className="cat" d="m203 136 3-17 13 9h13l14-9 2 18c1 10-9 16-22 16s-24-7-23-17Z"/><path className="cat-face" d="m213 138 4 1m21-1-4 1m-11 5 3 2 3-2"/>
  </svg>
}
