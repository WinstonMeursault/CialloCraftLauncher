import assert from 'node:assert/strict'
import test from 'node:test'
import { acceptTaskSnapshot, formatGuiError, newestHistory, redactSensitive, retainBusyManagedEntries, taskProgress, type Bootstrap, type TaskSnapshot } from '../src/types.ts'

function snapshot(id: number, revision: number): TaskSnapshot {
  return { id, revision, kind: 'probe', phase: 'running', state: 'running', completed: 0, total: 10, logs: [], error: null, error_code: null, pid: null, result: null, authorization: null }
}

test('accepts a newer revision of the current task', () => {
  const current = snapshot(4, 7)
  const incoming = snapshot(4, 8)

  assert.equal(acceptTaskSnapshot(current, incoming), incoming)
})

test('ignores stale revisions and snapshots from older tasks', () => {
  const current = snapshot(4, 7)

  assert.equal(acceptTaskSnapshot(current, snapshot(4, 6)), current)
  assert.equal(acceptTaskSnapshot(current, snapshot(3, 99)), current)
})

test('accepts a new task even when its revision starts over', () => {
  const incoming = snapshot(5, 1)

  assert.equal(acceptTaskSnapshot(snapshot(4, 99), incoming), incoming)
})

test('a null event snapshot leaves the current snapshot unchanged', () => {
  const current = snapshot(4, 7)

  assert.equal(acceptTaskSnapshot(current, null), current)
})

test('unknown and invalid totals have no numeric progress', () => {
  assert.equal(taskProgress({ completed: 4, total: null }), null)
  assert.equal(taskProgress({ completed: 4, total: 0 }), null)
  assert.equal(taskProgress({ completed: 4, total: Number.NaN }), null)
  assert.equal(taskProgress({ completed: Number.NaN, total: 10 }), 0)
  assert.equal(taskProgress({ completed: 12, total: 10 }), 100)
})

test('history replaces stale revisions, sorts newest first, and retains eight tasks', () => {
  const old = Array.from({ length: 8 }, (_, index) => snapshot(index + 1, 1))
  const incoming = [snapshot(8, 0), snapshot(8, 2), snapshot(9, 1), snapshot(10, 1)]
  const history = newestHistory(old, incoming)

  assert.equal(history.length, 8)
  assert.deepEqual(history.map(item => item.id), [10, 9, 8, 7, 6, 5, 4, 3])
  assert.equal(history.find(item => item.id === 8)?.revision, 2)
})

test('keeps managed entries when a busy library refresh omits them', () => {
  const oldEntry = { key: 'managed:atelier', id: 'atelier', kind: 'managed' as const, minecraft: '1.21.1', loader: 'fabric', game_directory: '/games/atelier' }
  const current: Bootstrap = { root: '/games', root_exists: true, setup_required: false, config_path: '/config', launch_defaults: { java: null, max_memory_mb: 4096, auto_install_java: true, network_offline: false }, preferences: { theme: 'system', language: 'en' }, entries: [oldEntry], warnings: [], prototype: true }
  const incomplete: Bootstrap = { ...current, entries: [], warnings: ['Managed instances are busy or unreadable; retry after other operations finish'] }

  assert.deepEqual(retainBusyManagedEntries(current, incomplete).entries, [oldEntry])
  assert.deepEqual(retainBusyManagedEntries(current, { ...incomplete, warnings: [] }).entries, [])
})

test('GUI errors retain structured codes and use a safe fallback', () => {
  assert.equal(formatGuiError({ code: 'task_busy', message: 'Please wait' }, 'Unknown error'), 'task_busy: Please wait')
  assert.equal(formatGuiError(new Error('Failed'), 'Unknown error'), 'Failed')
  assert.equal(formatGuiError({ unexpected: true }, 'Unknown error'), 'Unknown error')
})

test('dry-run plans redact sensitive fields and token arguments', () => {
  assert.deepEqual(redactSensitive({ prepared: { access_token: 'secret', arguments: ['--accessToken', 'secret', 'client_secret=secret'] } }), {
    prepared: { access_token: '[redacted]', arguments: ['--accessToken', '[redacted]', 'client_secret=[redacted]'] },
  })
})
