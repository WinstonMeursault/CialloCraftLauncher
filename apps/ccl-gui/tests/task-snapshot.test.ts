import assert from 'node:assert/strict'
import test from 'node:test'
import { acceptTaskSnapshot, boundLogLines, formatGuiError, GUI_LOG_MAX_BYTES, GUI_LOG_MAX_LINES, newestHistory, redactSensitive, retainBusyEntries, resolveSelectedInstance, taskProgress, type Bootstrap, type TaskSnapshot } from '../src/types.ts'

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

test('keeps entries with active revision storage when a busy library refresh omits them', () => {
  const oldEntry = { key: 'instance:' + 'a'.repeat(64), id: 'atelier', display_name: 'Atelier', has_revision_storage: true, launch_version: '1.21.1', minecraft: '1.21.1', loader: 'fabric', game_directory: '/games/atelier' }
  const current: Bootstrap = { root: '/games', root_exists: true, setup_required: false, config_path: '/config', launch_defaults: { java: null, max_memory_mb: 4096, auto_install_java: true, network_offline: false }, preferences: { theme: 'system', language: 'en', reduce_transparency: false, reduce_motion: false, last_selected_instance: null, background_image: null }, entries: [oldEntry], warnings: [], hidden_entries: 0 }
  const incomplete: Bootstrap = { ...current, entries: [], warnings: ['Instance revision metadata is busy or unreadable; retry after other operations finish'] }

  assert.deepEqual(retainBusyEntries(current, incomplete).entries, [oldEntry])
  assert.deepEqual(retainBusyEntries(current, { ...incomplete, warnings: [] }).entries, [])
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

test('100,000-line pressure fixture retains only the bounded tail and byte budget', () => {
  const input = Array.from({ length: 100_000 }, (_, index) => `${index.toString().padStart(6, '0')} ${'日志'.repeat(500)}`)
  const started = performance.now()
  const output = boundLogLines(input)
  const elapsedMs = performance.now() - started
  const bytes = new TextEncoder().encode(output.join('\n')).byteLength

  assert.ok(output.length > 0 && output.length <= GUI_LOG_MAX_LINES)
  assert.ok(Number(output[0]!.slice(0, 5)) > 5_000)
  assert.match(output.at(-1)!, /^099999 /)
  assert.ok(bytes <= GUI_LOG_MAX_BYTES)
  assert.ok(output.every(line => new TextEncoder().encode(line).byteLength <= 8 * 1024))
  console.log(JSON.stringify({ workload: '100000-lines', retained_lines: output.length, retained_utf8_bytes: bytes, reducer_elapsed_ms: Number(elapsedMs.toFixed(2)), scope: 'log state bound only; not a GUI measurement' }))
})

test('a single oversized Unicode line is truncated on a UTF-8 boundary', () => {
  const output = boundLogLines([`${'界'.repeat(10_000)}tail`])
  const bytes = new TextEncoder().encode(output[0]!).byteLength
  assert.ok(bytes <= 8 * 1024)
  assert.equal(output[0]!.endsWith('tail'), true)
  assert.doesNotThrow(() => new TextDecoder('utf-8', { fatal: true }).decode(new TextEncoder().encode(output[0]!)))
})

// Saved display names can collide; only the opaque library key chooses a game.
test('instance selection requires the stable key and never falls back to another game', () => {
  const first = { key: 'profile:a', id: 'a', display_name: '酒狐的奇妙冒险 with spaces', has_revision_storage: false, launch_version: 'a', minecraft: '1.20.1', loader: 'forge', game_directory: '/games/a' }
  const second = { ...first, key: 'profile:b', id: 'b', game_directory: '/games/b' }
  assert.equal(resolveSelectedInstance([first, second], second.key), second)
  assert.equal(resolveSelectedInstance([first, second], second.display_name), null)
  assert.equal(resolveSelectedInstance([first], second.key), null)
  assert.equal(resolveSelectedInstance([first], null), null)
})
