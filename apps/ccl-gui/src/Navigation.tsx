import { useEffect, useRef, useState, type ReactNode } from 'react'
import type { Language } from './types'

export type Page = 'home' | 'library' | 'discover' | 'accounts' | 'logs' | 'settings'
export type IconName = Page | 'folder' | 'search' | 'menu' | 'back' | 'more' | 'play' | 'chevron' | 'plus' | 'import' | 'refresh'
export function Icon({ name }: { name: IconName }) {
  const paths: Record<IconName, ReactNode> = {
    home: <><path d="m3 11 9-8 9 8v9a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z"/></>,
    library: <><rect x="3" y="4" width="5" height="16" rx="1"/><rect x="10" y="4" width="5" height="16" rx="1"/><path d="m17 5 4-1 3 15-4 1z"/></>,
    discover: <><path d="M12 3v12m-4-4 4 4 4-4M4 16v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4"/></>,
    accounts: <><circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/></>,
    logs: <><path d="M6 3h9l4 4v14H6zM9 11h7M9 15h7M9 18h4"/></>,
    settings: <><path d="m10 3-1 3-3 1-3 3 2 2-1 3 3 3 3-1 2 2 3-1 1-3 3-1 1-3-2-2V6l-3-2-3 1z"/><circle cx="12" cy="11" r="3"/></>,
    folder: <path d="M3 6a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>,
    search: <><circle cx="10.8" cy="10.8" r="6.8"/><path d="m16 16 4.5 4.5"/></>,
    menu: <path d="M4 6h16M4 12h16M4 18h16"/>, back: <path d="m14 5-7 7 7 7"/>,
    more: <><circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/></>,
    play: <path d="m8 4 12 8-12 8z"/>, chevron: <path d="m6 9 6 6 6-6"/>,
    refresh: <><path d="M20 8a8 8 0 1 0 0 8M20 3v5h-5"/></>,
    plus: <path d="M12 4v16M4 12h16"/>, import: <><path d="M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5"/></>,
  }
  return <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round">{paths[name]}</svg>
}

export function Navigation({ page, language, setup, onPage }: { page: Page; language: Language; setup: boolean; onPage: (page: Page) => void }) {
  const [expanded, setExpanded] = useState(false), [pinned, setPinned] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const rail = useRef<HTMLElement>(null)
  const hovering = useRef(false)
  const keyboardFocus = useRef(false)
  const zh = language === 'zh-CN'
  const labels: Record<Page, string> = zh
    ? { home: '开始游戏', library: '游戏库', discover: '下载中心', accounts: '账户', logs: '日志与诊断', settings: '设置' }
    : { home: 'Play', library: 'Library', discover: 'Downloads', accounts: 'Accounts', logs: 'Logs & diagnostics', settings: 'Settings' }
  const clear = () => { clearTimeout(timer.current) }
  useEffect(() => {
    const keyboard = (event: KeyboardEvent) => { if (event.key === 'Tab') keyboardFocus.current = true }
    const pointer = () => { keyboardFocus.current = false }
    document.addEventListener('keydown', keyboard)
    document.addEventListener('pointerdown', pointer)
    return () => { clearTimeout(timer.current); document.removeEventListener('keydown', keyboard); document.removeEventListener('pointerdown', pointer) }
  }, [])
  const collapse = () => {
    clear()
    timer.current = setTimeout(() => { if (!(keyboardFocus.current && rail.current?.contains(document.activeElement)) && !hovering.current) setExpanded(false) }, 220)
  }
  const item = (key: Page) => <button key={key} className={`nav-item${page === key ? ' active' : ''}`} aria-label={labels[key]} aria-current={page === key ? 'page' : undefined} disabled={setup && ['discover', 'accounts'].includes(key)} onClick={() => { onPage(key); setPinned(false) }}>
    <span className="nav-icon"><Icon name={key}/></span><span className="nav-label">{labels[key]}</span>
  </button>
  return <aside ref={rail} className={`sidebar${expanded || pinned ? ' expanded' : ''}`} aria-label={zh ? '导航' : 'Navigation'} onPointerEnter={event => {
    if (event.pointerType === 'touch') return
    hovering.current = true; clear(); timer.current = setTimeout(() => setExpanded(true), 180)
  }} onPointerLeave={() => { hovering.current = false; collapse() }} onFocus={() => { if (keyboardFocus.current) { clear(); setExpanded(true) } }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) collapse() }} onKeyDown={event => { if (event.key === 'Escape') { setPinned(false); setExpanded(false); (document.activeElement as HTMLElement)?.blur() } }}>
    <button className="brand" aria-label={zh ? '展开或收起导航' : 'Expand or collapse navigation'} aria-expanded={expanded || pinned} onClick={() => { clear(); setPinned(value => !value); setExpanded(false) }}><span className="brand-short">CCL</span><span className="nav-label brand-name">Launcher</span></button>
    <nav>{(['home', 'library', 'discover', 'accounts'] as const).map(item)}</nav>
    <nav className="sidebar-bottom">{(['logs', 'settings'] as const).map(item)}</nav>
  </aside>
}
