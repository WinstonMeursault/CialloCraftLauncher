import assert from 'node:assert/strict'
import test from 'node:test'
import { onlineRequestFingerprint, resolveRequestFingerprint } from '../src/online-request-model.ts'

const resolve = (version: string) => ({
  operation: 'resolve' as const,
  provider: 'modrinth',
  project: 'project-1',
  minecraft: '1.21.1',
  loader: 'fabric',
  version,
})

test('resolve request identity changes with selected version and every resolution input', () => {
  const original = resolveRequestFingerprint(resolve('v1'))
  assert.notEqual(resolveRequestFingerprint(resolve('v2')), original)
  assert.notEqual(resolveRequestFingerprint({ ...resolve('v1'), loader: 'forge' }), original)
  assert.notEqual(resolveRequestFingerprint({ ...resolve('v1'), project: 'project-2' }), original)
  assert.notEqual(resolveRequestFingerprint({ ...resolve('v1'), minecraft: '1.20.1' }), original)
})

test('request fingerprints are stable when object keys are reordered', () => {
  assert.equal(
    onlineRequestFingerprint({ operation: 'resolve', input: { version: 'v1', provider: 'modrinth' } }),
    onlineRequestFingerprint({ input: { provider: 'modrinth', version: 'v1' }, operation: 'resolve' }),
  )
})
