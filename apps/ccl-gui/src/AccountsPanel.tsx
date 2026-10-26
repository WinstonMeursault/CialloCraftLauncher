import { useState } from 'react'
import './AccountsPanel.css'
import type { Language } from './types'

export interface SavedAccount {
  id: string
  username: string
  uuid: string
  account_type: string
}

export interface AccountsPanelProps {
  language: Language
  accounts: SavedAccount[]
  selected: string | null
  disabled?: boolean
  onAddOffline: (username: string) => void | Promise<void>
  onSelect: (id: string) => void | Promise<void>
  onRemove: (id: string) => void | Promise<void>
}

/** Account tab content. Microsoft sign-in is intentionally unavailable in this build. */
export function AccountsPanel({ language, accounts, selected, disabled = false, onAddOffline, onSelect, onRemove }: AccountsPanelProps) {
  const [username, setUsername] = useState('')
  const [error, setError] = useState('')
  const [addOpen, setAddOpen] = useState(accounts.length === 0)
  const zh = language === 'zh-CN'
  const canAdd = /^[A-Za-z0-9_]{1,16}$/.test(username.trim())

  async function add() {
    const name = username.trim()
    if (!/^[A-Za-z0-9_]{1,16}$/.test(name)) {
      setError(zh ? '请输入 1–16 位英文字母、数字或下划线。' : 'Use 1–16 letters, numbers, or underscores.')
      return
    }
    setError('')
    try {
      await onAddOffline(name)
      setUsername('')
      setAddOpen(false)
    } catch {
      setError(zh ? '无法保存此账户，请检查玩家名后重试。' : 'Could not save this account. Check the player name and try again.')
    }
  }

  async function selectAccount(id: string) {
    setError('')
    try { await onSelect(id) } catch { setError(zh ? '无法选择此账户，请重试。' : 'Could not select this account. Please try again.') }
  }

  return <section className="saved-accounts">
    <header className="saved-accounts-head"><h1>{zh ? '账户' : 'Accounts'}</h1></header>
    <div className="saved-accounts-list" aria-label={zh ? '已保存的账户' : 'Saved accounts'}>
      {accounts.length === 0 ? <div className="saved-accounts-empty"><strong>{zh ? '还没有保存的账户' : 'No saved accounts yet'}</strong><span>{zh ? '展开“添加离线账户”即可开始。' : 'Expand “Add offline account” to get started.'}</span></div> : accounts.map(account => {
        const active = account.id === selected
        const remove = async () => {
          const prompt = zh ? `确定移除账户“${account.username}”吗？` : `Remove the account “${account.username}”?`
          if (!window.confirm(prompt)) return
          setError('')
          try { await onRemove(account.id) }
          catch { setError(zh ? '无法移除此账户，请重试。' : 'Could not remove this account. Please try again.') }
        }
        return <article className={`saved-accounts-row${active ? ' is-selected' : ''}`} key={account.id}>
          <div className="saved-accounts-info"><strong>{account.username}</strong><span>{account.account_type === 'offline' ? (zh ? '离线账户' : 'Offline account') : (zh ? 'Microsoft 账户' : 'Microsoft account')}</span></div>
          {active ? <span className="saved-accounts-current">{zh ? '已选中' : 'Selected'}</span> : <button type="button" disabled={disabled} onClick={() => void selectAccount(account.id)}>{zh ? '选择' : 'Select'}</button>}
          <button className="saved-accounts-remove" type="button" disabled={disabled} aria-label={zh ? `移除 ${account.username}` : `Remove ${account.username}`} onClick={remove}>{zh ? '移除' : 'Remove'}</button>
        </article>
      })}
    </div>
    {error && <p className="saved-accounts-error" role="alert">{error}</p>}
    <details className="saved-accounts-add" open={addOpen} onToggle={event => setAddOpen(event.currentTarget.open)}>
      <summary>{zh ? '添加离线账户' : 'Add offline account'}</summary>
      <div className="saved-accounts-form">
        <label htmlFor="offline-account-name">{zh ? '玩家名' : 'Player name'}</label>
        <input id="offline-account-name" autoComplete="off" maxLength={16} value={username}
          placeholder={zh ? '玩家名' : 'Player name'} disabled={disabled}
          onChange={event => { setUsername(event.target.value); setError('') }}
          onKeyDown={event => { if (event.key === 'Enter') void add() }} />
        <button type="button" disabled={disabled || !canAdd} onClick={() => void add()}>{zh ? '添加账户' : 'Add account'}</button>
      </div>
    </details>
    <aside className="saved-accounts-unavailable"><p>{zh ? 'Microsoft 登录暂不可用。已保存的账户记录会保留。' : 'Microsoft sign-in is temporarily unavailable. Saved account records are retained.'}</p></aside>
  </section>
}

export default AccountsPanel
