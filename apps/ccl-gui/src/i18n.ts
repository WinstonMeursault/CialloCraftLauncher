import type { Language } from './types'

const messages = {
  en: {
    brand: 'CialloCraftLauncher', tagline: 'Your little corner of Minecraft.', library: 'Library', discover: 'Discover', accounts: 'Accounts', tasks: 'Tasks', logs: 'Logs & diagnostics', settings: 'Settings',
    welcome: 'MAKE YOURSELF AT HOME', heading: 'A world worth returning to.', subheading: 'Your Minecraft library, rooted on this device.', instances: 'Instances', search: 'Filter by ID, version, or loader', all: 'All', managed: 'Managed', vanilla: 'Shared vanilla', empty: 'No matching entries', emptyHint: 'Choose a game root or check that it contains Minecraft data.', root: 'Game root', chooseRoot: 'Choose root…', changeRoot: 'Change root', rootMissing: 'This root does not exist yet.', rootPath: 'Root path', configPath: 'Configuration', warnings: 'Startup notes', selected: 'Selected entry', gameVersion: 'Minecraft version', loader: 'Loader', kind: 'Type', gameDirectory: 'Game directory', id: 'ID', unavailable: 'Not yet available', soon: 'This action is outside the current prototype.', launch: 'Launch game', launchHint: 'Game launching is not connected in this prototype.', create: 'Create instance', importPack: 'Import pack',
    appearance: 'Appearance', appearanceCopy: 'Choose how CCL looks on this device.', theme: 'Theme', system: 'System', light: 'Light · Lilac', dark: 'Dark · Indigo', language: 'Language', english: 'English', chinese: '简体中文', save: 'Save preferences', saved: 'Preferences saved.', unsaved: 'Unsaved changes',
    taskIntro: 'Task center', probe: 'Start synthetic task probe', probeDescription: 'A clearly labeled pressure fixture: synthetic logs only, up to 10,000 lines per minute for 10 minutes. It does not download or install anything.', noTask: 'No active task', noTaskCopy: 'Long-running work will appear here as it is connected.', startProbe: 'Start probe', cancel: 'Cancel task', starting: 'Starting…', progress: 'Progress', logHeading: 'Latest log lines', noLogs: 'Waiting for synthetic log lines…',
    notConnected: 'Not connected', notConnectedCopy: 'Open this screen from the Tauri desktop app to access the local game root.', retry: 'Retry', loading: 'Loading local library…', error: 'Something went wrong', unknownError: 'Unknown error', unsupported: 'Not yet available', rootExists: 'Available', rootDoesNotExist: 'Not found', prototype: 'G0 prototype', taskSynthetic: 'Synthetic task fixture', state: { running: 'Running', cancelling: 'Cancelling', succeeded: 'Completed', failed: 'Failed', cancelled: 'Cancelled' },
  },
  'zh-CN': {
    brand: 'CialloCraftLauncher', tagline: '属于你的 Minecraft 小天地。', library: '游戏库', discover: '下载中心', accounts: '账户', tasks: '任务中心', logs: '日志与诊断', settings: '设置',
    welcome: '欢迎回到自己的小天地', heading: '总有一个世界，值得回来。', subheading: '此设备上的 Minecraft 游戏库。', instances: '实例', search: '按 ID、版本或加载器筛选', all: '全部', managed: '托管实例', vanilla: '共享原版', empty: '没有匹配的条目', emptyHint: '选择一个游戏根目录，或确认目录中已有 Minecraft 数据。', root: '游戏根目录', chooseRoot: '选择根目录…', changeRoot: '更改根目录', rootMissing: '此根目录尚不存在。', rootPath: '根目录路径', configPath: '配置文件', warnings: '启动提示', selected: '选中条目', gameVersion: 'Minecraft 版本', loader: '加载器', kind: '类型', gameDirectory: '游戏目录', id: 'ID', unavailable: '暂不可用', soon: '此操作尚未包含在当前原型中。', launch: '启动游戏', launchHint: '此原型尚未接入游戏启动。', create: '创建实例', importPack: '导入整合包',
    appearance: '外观', appearanceCopy: '选择此设备上的 CCL 外观。', theme: '主题', system: '跟随系统', light: '浅色 · 晴昼淡紫', dark: '深色 · 星夜靛蓝', language: '语言', english: 'English', chinese: '简体中文', save: '保存偏好设置', saved: '偏好设置已保存。', unsaved: '有未保存的更改',
    taskIntro: '任务中心', probe: '启动合成任务探针', probeDescription: '明确标记的压力夹具：仅生成合成日志，最多持续 10 分钟、每分钟 10,000 行。不会下载或安装内容。', noTask: '当前没有进行中的任务', noTaskCopy: '接入长时间任务后，它们会显示在这里。', startProbe: '启动探针', cancel: '取消任务', starting: '正在启动…', progress: '进度', logHeading: '最新日志', noLogs: '等待合成日志…',
    notConnected: '未连接桌面后端', notConnectedCopy: '请在 Tauri 桌面应用中打开此页面，以访问本机游戏目录。', retry: '重试', loading: '正在读取本地游戏库…', error: '发生错误', unknownError: '未知错误', unsupported: '暂不可用', prototype: 'G0 原型', taskSynthetic: '合成任务夹具', state: { running: '运行中', cancelling: '正在取消', succeeded: '已完成', failed: '失败', cancelled: '已取消' },
  },
} as const

export type Messages = (typeof messages)['en']
export function getMessages(language: Language): Messages {
  return messages[language] as unknown as Messages
}
