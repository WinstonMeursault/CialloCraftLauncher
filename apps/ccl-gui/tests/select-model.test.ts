import assert from 'node:assert/strict'
import test from 'node:test'
import { filterOptions, menuPosition, nextOption } from '../src/select-model.ts'

const options = [
  { value: '', label: 'Choose', disabled: true },
  { value: '1.21.1', label: '原版 生存 · Release', disabled: false },
  { value: 'snapshot', label: 'Snapshot 26w01a', disabled: false },
]

test('search matches Unicode labels and stable values without changing options', () => {
  assert.deepEqual(filterOptions(options, ' 原版 '), [options[1]])
  assert.deepEqual(filterOptions(options, '１.２１.１'), [options[1]])
  assert.deepEqual(filterOptions(options, 'SNAPSHOT'), [options[2]])
  assert.deepEqual(filterOptions(options, 'missing'), [])
  assert.deepEqual(filterOptions(options, ''), options)
})

test('keyboard navigation wraps and skips disabled options, including an empty value', () => {
  assert.equal(nextOption(options, null, 1), '1.21.1')
  assert.equal(nextOption(options, null, -1), 'snapshot')
  assert.equal(nextOption(options, 'snapshot', 1), '1.21.1')
  assert.equal(nextOption(options, '1.21.1', -1), 'snapshot')
  assert.equal(nextOption([{ value: '', label: 'Any', disabled: false }], null, 1), '')
  assert.equal(nextOption(options.slice(0, 1), null, 1), null)
  assert.equal(nextOption([], null, -1), null)
})

test('popup opens downward when there is room and upward at the bottom edge', () => {
  const viewport = { width: 800, height: 600 }
  const down = menuPosition({ top: 50, bottom: 86, left: 100, width: 240 }, viewport, 320)
  assert.equal(down.top, 92)
  assert.equal(down.bottom, undefined)
  assert.equal(down.maxHeight, 320)
  const up = menuPosition({ top: 540, bottom: 576, left: 100, width: 240 }, viewport, 320)
  assert.equal(up.bottom, 66)
  assert.equal(up.top, undefined)
  assert.equal(up.maxHeight, 320)
  // A short menu fits below even when a full-height menu would need to flip.
  const short = menuPosition({ top: 400, bottom: 436, left: 100, width: 240 }, viewport, 114)
  assert.equal(short.top, 442)
  assert.equal(short.bottom, undefined)
})

test('popup fits narrow windows and remains within viewport bounds', () => {
  const position = menuPosition({ top: 90, bottom: 126, left: 300, width: 600 }, { width: 360, height: 220 }, 320)
  assert.equal(position.width, 344)
  assert.equal(position.left, 8)
  assert.equal(position.maxHeight, 80)
  assert.equal(position.top, 132)
})
