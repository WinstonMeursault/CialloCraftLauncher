import { Select } from './Select'
import { useState, type FormEvent } from 'react'
import { Icon } from './Navigation'
import { ModalFrame } from './Dialog'
import type { AccountListing, Language, LibraryEntry, TaskSnapshot } from './types'
import { taskIsActive } from './types'

export function InstanceIcon({ entry, large = false }: { entry: LibraryEntry; large?: boolean }) {
  // A local vector emblem is the fallback until an instance supplies artwork.
  return <span className={`instance-icon${large ? ' large' : ''}`} aria-hidden="true" data-tone={Array.from(entry.key).reduce((n, c) => n + c.codePointAt(0)!, 0) % 4}><svg viewBox="0 0 48 48" fill="none"><path d="m24 7 16 9v17l-16 9-16-9V16z" fill="currentColor" opacity=".14"/><path d="m8 16 16 9 16-9M24 25v17M8 16l16-9 16 9v17l-16 9-16-9z" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round"/></svg></span>
}

export function Home({ language, entries, selected, accountListing, accountId, disabled, task, launchPending, missingSelection, onSelect, onAccount, onAccounts, onManage, onCreate, onImport, onLibrary, onLaunch, onProgress }: {
  language: Language; entries: LibraryEntry[]; selected: LibraryEntry | null; accountListing: AccountListing; accountId: string; disabled: boolean; task: TaskSnapshot | null; launchPending: boolean; missingSelection: boolean;
  onSelect: (key: string) => void; onAccount: (id: string) => void; onAccounts: () => void; onManage: () => void; onCreate: () => void; onImport: () => void; onLibrary: () => void; onLaunch: (event: FormEvent) => void; onProgress: () => void
}) {
  const zh = language === 'zh-CN', text = (cn: string, en: string) => zh ? cn : en
  const [picker, setPicker] = useState(false), [query, setQuery] = useState('')
  const visible = entries.filter(entry => `${entry.display_name} ${entry.minecraft} ${entry.loader ?? ''}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  const running = task?.kind === 'launch' && taskIsActive(task) && task.pid !== null
  const preparing = launchPending || task?.kind === 'launch' && taskIsActive(task) && task.pid === null
  return <section className="home-page" aria-label={text('开始游戏', 'Play')}>
    <div className="launch-cluster">
      {selected ? <>
        <div className="home-instance"><InstanceIcon entry={selected} large/><div className="home-instance-copy"><h1 title={selected.display_name}>{selected.display_name}</h1><button className="home-text-button" disabled={disabled} onClick={() => { setQuery(''); setPicker(true) }}>{text('切换实例', 'Switch instance')}<Icon name="chevron"/></button></div><button className="home-icon-button" aria-label={text('管理此实例', 'Manage this instance')} title={text('管理此实例', 'Manage this instance')} onClick={onManage}><Icon name="more"/></button></div>
        <form onSubmit={onLaunch} className="home-launch-form"><label className="home-account"><Icon name="accounts"/><span className="sr-only">{text('账户', 'Account')}</span><Select aria-label={text('账户', 'Account')} value={accountId} disabled={disabled} onValueChange={value => value === '__manage__' ? onAccounts() : onAccount(value)}><option value="">{text('选择账户', 'Choose account')}</option>{accountListing.accounts.map(account => <option key={account.id} value={account.id}>{account.username} · {account.account_type === 'offline' ? text('离线', 'Offline') : 'Microsoft'}</option>)}<option value="__manage__">{text('管理账户…', 'Manage accounts…')}</option></Select></label>
          {running || preparing ? <button type="button" className="button primary home-launch" onClick={onProgress}>{running ? text('游戏运行中', 'Game running') : text('正在准备…', 'Preparing…')}</button> : !accountId ? <button type="button" className="button primary home-launch" disabled={disabled} onClick={onAccounts}>{text('选择账户', 'Choose account')}</button> : <button className="button primary home-launch" disabled={disabled}><Icon name="play"/>{text('启动游戏', 'Play')}</button>}
        </form>
      </> : <div className="home-empty"><h1>{entries.length ? text('选择要继续的实例', 'Choose an instance') : text('添加第一个实例', 'Add an instance')}</h1>{entries.length ? <>{missingSelection && <p>{text('之前选择的实例当前不可用。', 'The previously selected instance is unavailable.')}</p>}<button className="button primary" disabled={disabled} onClick={() => { setQuery(''); setPicker(true) }}>{text('选择实例', 'Choose instance')}</button></> : <div className="heading-actions"><button className="button secondary" disabled={disabled} onClick={onImport}>{text('导入整合包', 'Import pack')}</button><button className="button primary" disabled={disabled} onClick={onCreate}>{text('创建实例', 'Create instance')}</button></div>}</div>}
    </div>
    {picker && <ModalFrame placement="switcher" title={text('切换实例', 'Switch instance')} close={() => setPicker(false)} closeLabel={text('关闭', 'Close')}><label className="search-field"><Icon name="search"/><input aria-label={text('搜索实例', 'Search instances')} placeholder={text('搜索实例', 'Search instances')} value={query} onChange={event => setQuery(event.target.value)}/></label><div className="switcher-list">{visible.map(entry => <button className={`instance-row${selected?.key === entry.key ? ' selected' : ''}`} key={entry.key} aria-pressed={selected?.key === entry.key} disabled={disabled} onClick={() => { onSelect(entry.key); setPicker(false) }}><InstanceIcon entry={entry}/><span className="instance-row-copy"><strong>{entry.display_name}</strong><small>{entry.minecraft} · {entry.loader ?? text('原版', 'Vanilla')}</small></span></button>)}{!visible.length && <p className="muted-copy">{text('未找到实例。', 'No instances found.')}</p>}</div><div className="modal-actions"><button className="button secondary" onClick={() => { setPicker(false); onLibrary() }}>{text('打开游戏库', 'Open library')}</button></div></ModalFrame>}
  </section>
}
