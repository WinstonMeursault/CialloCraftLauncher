import assert from 'node:assert/strict'
import test from 'node:test'
import { acceptTaskSnapshot, type TaskSnapshot } from '../src/types.ts'

function snapshot(id: number, revision: number): TaskSnapshot {
  return { id, revision, state: 'running', completed: 0, total: 10, logs: [], error: null }
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
