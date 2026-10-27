import { Select } from './Select'
import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { formatGuiError, type Language, type LibraryEntry, type MemoryPolicy } from './types'

/** Explicit save: opening or launching a profile never writes a memory override. */
export function InstanceMemorySettings({ entry, language, disabled, onSaved }: { entry: LibraryEntry; language: Language; disabled: boolean; onSaved: () => void }) {
  const zh = language === 'zh-CN'
  const [saved, setSaved] = useState<MemoryPolicy | null>(entry.memory_override)
  const [mode, setMode] = useState<'inherit' | 'auto' | 'manual'>(entry.memory_override === null ? 'inherit' : entry.memory_override === 'auto' ? 'auto' : 'manual')
  const [manual, setManual] = useState(String(typeof entry.memory_override === 'number' ? entry.memory_override : 4096))
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  useEffect(() => {
    setSaved(entry.memory_override)
    setMode(entry.memory_override === null ? 'inherit' : entry.memory_override === 'auto' ? 'auto' : 'manual')
    if (typeof entry.memory_override === 'number') setManual(String(entry.memory_override))
    setError(''); setNotice('')
  }, [entry.key, entry.memory_override])
  const value = mode === 'inherit' ? null : mode === 'auto' ? 'auto' : Number(manual)
  const valid = typeof value !== 'number' || (Number.isSafeInteger(value) && value >= 256 && value <= 131072)
  const save = async () => {
    if (!valid || pending) return
    setPending(true); setError(''); setNotice('')
    try {
      await invoke('library_update', { request: { operation: 'memory', key: entry.key, memory: value, expected: saved } })
      setSaved(value); setNotice(zh ? '内存设置已保存。' : 'Memory setting saved.'); onSaved()
    } catch (cause) { setError(formatGuiError(cause, zh ? '内存设置保存失败' : 'Could not save memory setting')) }
    finally { setPending(false) }
  }
  return <section className="instance-memory-settings" aria-label={zh ? '实例内存' : 'Instance memory'}>
    <label className="field"><span>{zh ? '内存分配' : 'Memory allocation'}</span><Select value={mode} disabled={disabled || pending} onValueChange={value => { setMode(value as typeof mode); setNotice('') }}><option value="inherit">{zh ? '跟随全局' : 'Use global setting'}</option><option value="auto">{zh ? '自动' : 'Automatic'}</option><option value="manual">{zh ? '手动' : 'Manual'}</option></Select></label>
    {mode === 'manual' && <label className="field"><span>{zh ? '最大堆内存（MiB）' : 'Maximum heap (MiB)'}</span><input type="number" min="256" max="131072" step="1" value={manual} disabled={disabled || pending} onChange={event => { setManual(event.target.value); setNotice('') }}/></label>}
    {mode === 'auto' && <p className="muted-copy">{zh ? '启动时按可用内存计算，自动分配最高 16 GiB。' : 'Calculated from available memory at launch, up to 16 GiB.'}</p>}
    {!valid && <p role="alert">{zh ? '请输入 256–131072 MiB 之间的整数。' : 'Enter a whole number from 256 to 131072 MiB.'}</p>}
    <button className="button secondary" disabled={disabled || pending || !valid || value === saved} onClick={() => void save()}>{zh ? '保存此实例内存设置' : 'Save instance memory setting'}</button>
    {notice && <p role="status">{notice}</p>}{error && <p role="alert" className="alert">{error}</p>}
  </section>
}
