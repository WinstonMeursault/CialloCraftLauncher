export interface SelectOption { value: string; label: string; disabled: boolean }

export function filterOptions(options: SelectOption[], query: string) {
  const needle = query.trim().normalize('NFKC').toLocaleLowerCase()
  return options.filter(option => `${option.label} ${option.value}`.normalize('NFKC').toLocaleLowerCase().includes(needle))
}

export function nextOption(options: SelectOption[], current: string | null, direction: 1 | -1) {
  const enabled = options.filter(option => !option.disabled)
  if (!enabled.length) return null
  const index = enabled.findIndex(option => option.value === current)
  return enabled[(index < 0 ? (direction === 1 ? 0 : enabled.length - 1) : (index + direction + enabled.length) % enabled.length)].value
}

export function menuPosition(anchor: { top: number; bottom: number; left: number; width: number }, viewport: { width: number; height: number }, desiredHeight: number) {
  const margin = 8, gap = 6
  const below = Math.max(0, viewport.height - anchor.bottom - margin - gap)
  const above = Math.max(0, anchor.top - margin - gap)
  const upward = below < desiredHeight && above > below
  const maxHeight = Math.min(desiredHeight, upward ? above : below)
  const width = Math.min(Math.max(anchor.width, 180), viewport.width - 2 * margin)
  return { left: Math.max(margin, Math.min(anchor.left, viewport.width - width - margin)), width, maxHeight,
    ...(upward ? { bottom: viewport.height - anchor.top + gap } : { top: anchor.bottom + gap }) }
}
