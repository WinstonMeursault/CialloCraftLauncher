export type OnlineResolveRequest = {
  operation: 'resolve'
  provider: string
  project: string
  minecraft: string
  loader: string | null
  version: string
}

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue)
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, stableValue(nested)]),
    )
  }
  return value
}

/** Stable identity for the full request; used to prevent stale plans from enabling install. */
export function onlineRequestFingerprint(request: Record<string, unknown>): string {
  return JSON.stringify(stableValue(request))
}

export function resolveRequestFingerprint(request: OnlineResolveRequest): string {
  return onlineRequestFingerprint(request)
}
