import { ModalFrame } from './Dialog'
import { ResultView } from './Advanced'
import { getMessages } from './i18n'
import { taskIsActive, taskProgress, redactSensitive, type Language, type TaskSnapshot } from './types'

type Props = { task: TaskSnapshot; language: Language; visible: boolean; onVisibility: (visible: boolean) => void; onCancel: () => void; onLogs: () => void }
export function ProgressDialog({ task, language, visible, onVisibility, onCancel, onLogs }: Props) {
  const zh = language === 'zh-CN', t = getMessages(language)
  const kinds: Record<string, string> = zh
    ? { install: '安装游戏', import: '导入整合包', launch: '启动游戏', instance: '管理实例', online: '安装内容', update: '更新启动器', refresh: '刷新账户', login: '账户登录', log_pressure: '日志验证' }
    : { install: 'Install game', import: 'Import modpack', launch: 'Launch game', instance: 'Manage instance', online: 'Install content', update: 'Update launcher', refresh: 'Refresh account', login: 'Sign in', log_pressure: 'Log validation' }
  const active = taskIsActive(task), progress = taskProgress(task)
  const title = kinds[task.kind] ?? (zh ? '处理操作' : 'Operation')
  const stages: [string, string, string][] = [['download ', '正在下载文件', 'Downloading files'], ['install', '正在安装', 'Installing'], ['assets', '正在准备游戏资源', 'Preparing assets'], ['java', '正在准备 Java', 'Preparing Java'], ['import', '正在导入整合包', 'Importing modpack'], ['resolve', '正在解析依赖', 'Resolving dependencies'], ['prepare', '正在准备启动', 'Preparing launch'], ['online_content_apply', '正在安装内容', 'Installing content']]
  const stage = stages.find(([prefix]) => task.phase.toLowerCase().startsWith(prefix))
  const phase = task.state === 'cancelling' ? (zh ? '正在安全停止…' : 'Stopping safely…')
    : task.pid !== null && active ? (zh ? '游戏正在运行' : 'Game is running')
    : active ? (stage ? stage[zh ? 1 : 2] : (zh ? '正在处理，请稍候…' : 'Working, please wait…')) : t.state[task.state]
  if (!visible && !active) return null
  if (!visible) return <button className="operation-chip" onClick={() => onVisibility(true)}><span>{title} · {phase}</span>{active && progress !== null && <strong>{Math.round(progress)}%</strong>}</button>
  const result = task.result && typeof task.result === 'object' ? task.result as Record<string, unknown> : null
  const prepared = result?.prepared
  return <ModalFrame title={title} close={() => onVisibility(false)} closeLabel={active ? (zh ? '收起进度' : 'Minimize progress') : t.close}>
    <p role="status">{phase}</p>
    {active && <><div className="progress-track" role="progressbar" aria-label={t.progress} aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress === null ? undefined : Math.round(progress)}><span className={progress === null ? 'indeterminate-progress' : ''} style={progress === null ? undefined : { width: `${progress}%` }}/></div><p className="muted-copy">{progress === null ? t.totalUnknown : `${Math.round(progress)}%`}{task.total !== null ? ` · ${task.completed.toLocaleString()} / ${task.total.toLocaleString()}` : ''}</p></>}
    {task.error && <div role="alert" className="alert"><span>{task.error_code === 'invalid_launch_key' ? (zh ? '无法识别所选游戏条目，请刷新游戏库后重试。' : 'The selected game could not be resolved. Refresh the library and retry.') : task.error}</span></div>}
    {!active && task.state === 'succeeded' && <p>{zh ? '操作已完成。' : 'Operation completed.'}</p>}
    {prepared != null && <details><summary>{t.dryRun}</summary><ResultView value={redactSensitive(prepared)} language={language}/></details>}
    {task.logs.length > 0 && <details><summary>{t.logHeading}</summary><pre className="log-view">{task.logs.slice(-30).join('\n')}</pre></details>}
    <div className="modal-actions"><button className="button secondary" onClick={() => onVisibility(false)}>{active ? (zh ? '收起' : 'Minimize') : t.close}</button>{task.kind === 'launch' && <button className="button secondary" onClick={onLogs}>{zh ? '查看游戏日志' : 'Game logs'}</button>}{active && <button className="button danger" disabled={task.cancellable === false || task.state === 'cancelling'} onClick={onCancel}>{task.kind === 'launch' && task.pid !== null ? (zh ? '停止游戏' : 'Stop game') : t.cancel}</button>}</div>
    {active && task.cancellable === false && <p className="muted-copy">{zh ? '正在提交文件变更，请等待完成。' : 'File changes are being committed; wait for completion.'}</p>}
  </ModalFrame>
}
