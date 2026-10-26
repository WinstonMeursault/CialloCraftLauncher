import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { formatGuiError, type Language } from './types'

type Snapshot = { text: string; path: string; debug: boolean }
export function LauncherLogs({ language, active = true }: { language: Language; active?: boolean }) {
  const zh = language === 'zh-CN'
  const [index, setIndex] = useState(0), [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [notice, setNotice] = useState('')
  const load = useCallback(async () => {
    setBusy(true); setError('')
    try { setSnapshot(await invoke<Snapshot>('launcher_logs', { index })) }
    catch (cause) { setError(formatGuiError(cause, zh ? '无法读取启动器日志' : 'Could not read launcher logs')) }
    finally { setBusy(false) }
  }, [index, zh])
  useEffect(() => { if (active) void load() }, [load, active])
  const debug = async (enabled: boolean) => {
    setBusy(true); setError('')
    try { await invoke('launcher_log_debug', { enabled }); await load() }
    catch (cause) { setError(formatGuiError(cause, zh ? '无法保存日志设置' : 'Could not save logging preference')) }
    finally { setBusy(false) }
  }
  const exportLog = async () => {
    setBusy(true); setError(''); setNotice('')
    try { if (await invoke<boolean>('launcher_log_export', { index })) setNotice(zh ? '日志已导出到所选位置。' : 'Log exported to the selected location.') }
    catch (cause) { setError(formatGuiError(cause, zh ? '无法导出启动器日志' : 'Could not export launcher logs')) }
    finally { setBusy(false) }
  }
  const displayed = snapshot?.text.replace(/^(\d{13}) /gm, (_, timestamp: string) => `${new Date(Number(timestamp)).toLocaleString(zh ? 'zh-CN' : 'en')} `)
  return <section className="logs-panel"><h2>{zh ? 'CCL 启动器日志' : 'CCL launcher logs'}</h2><p className="muted-copy">{zh ? '记录启动、操作结果和错误码，不记录账户令牌、输入内容或游戏输出。最多保留 5 个文件，每个 5 MiB。' : 'Records startup, operation results and error codes, without account tokens, input values or game output. Retains up to five files of 5 MiB each.'}</p>
    <div className="log-controls"><label className="field"><span>{zh ? '日志文件' : 'Log file'}</span><select value={index} disabled={busy} onChange={event => setIndex(Number(event.target.value))}>{[0, 1, 2, 3, 4].map(value => <option key={value} value={value}>{value === 0 ? (zh ? '当前日志' : 'Current log') : (zh ? `历史日志 ${value}` : `Previous log ${value}`)}</option>)}</select></label><button className="button secondary" disabled={busy} onClick={() => void load()}>{zh ? '刷新' : 'Refresh'}</button><button className="button secondary" disabled={busy || !snapshot?.text} onClick={() => void exportLog()}>{zh ? '导出所显示日志' : 'Export displayed log'}</button><label className="check-row"><input type="checkbox" checked={snapshot?.debug ?? false} disabled={busy || !snapshot} onChange={event => void debug(event.target.checked)}/>{zh ? '记录调试事件' : 'Record debug events'}</label></div>
    {snapshot && <p className="settings-path">{snapshot.path}</p>}{error && <p role="alert" className="alert">{error}</p>}{notice && <p role="status">{notice}</p>}<pre className="run-log">{displayed || (zh ? '暂无启动器日志。' : 'No launcher logs yet.')}</pre>
  </section>
}
