import test from 'node:test'
import assert from 'node:assert/strict'
import { deletionConfirmation } from '../src/library-actions.ts'

test('deletion identifies the full instance parent including history', () => {
  const text = deletionConfirmation({delete_paths: ['/games/.ccl/instances/demo'], retained_paths: []}, 'en')
  assert.match(text, /revision history/)
  assert.match(text, /\/games\/\.ccl\/instances\/demo/)
  assert.doesNotMatch(text, /demo\/game/)
})

test('shared and custom runtime paths are explicitly preserved in both languages', () => {
  for (const language of ['en', 'zh-CN']) {
    const text = deletionConfirmation({delete_paths: ['/games/versions/demo'], retained_paths: ['/games/libraries', '/worlds/custom']}, language)
    assert.ok(text.indexOf('/worlds/custom') > text.indexOf(language === 'en' ? 'Preserved:' : '保留：'))
    assert.ok(text.indexOf('/games/versions/demo') < text.indexOf(language === 'en' ? 'Preserved:' : '保留：'))
  }
})
