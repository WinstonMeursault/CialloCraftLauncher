import type { LibraryEntry } from './types.ts'

/** Show owned deletion scope, never infer it from the working directory. */
export function deletionConfirmation(entry: Pick<LibraryEntry, 'delete_paths' | 'retained_paths'>, language: string): string {
  const zh = language === 'zh-CN'
  const removed = zh
    ? '将永久删除以下路径（包含其中的存档与历史版本）：\n'
    : 'Permanently delete these paths, including any saves and revision history within them:\n'
  const retained = zh ? '\n保留：\n' : '\nPreserved:\n'
  return removed + entry.delete_paths.join('\n') + (entry.retained_paths.length ? retained + entry.retained_paths.join('\n') : '')
}
