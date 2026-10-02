import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, writeFile, stat, symlink, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createStorage, STORAGE_LIMITS } from '../src/shims/storage.ts'

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'codewhale-plugin-storage-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const a = join(root, 'a'), b = join(root, 'b')
  await Promise.all([mkdir(a, { mode: 0o700 }), mkdir(b, { mode: 0o700 })])
  let live = true
  return { a, b, store: createStorage({ dataDir: a, isActive: () => live }), revoke: () => { live = false } }
}

const code = (expected) => (error) => error.code === expected

test('storage persists JSON across owner generations and deletes exact keys', async (t) => {
  const { a, store } = await fixture(t)
  assert.ok(Object.isFrozen(store))
  assert.equal(await store.get('absent'), undefined)
  await store.set('profile', { enabled: true, list: [null, 3, '你好'] })
  const restarted = createStorage({ dataDir: a, isActive: () => true })
  assert.deepEqual(await restarted.get('profile'), { enabled: true, list: [null, 3, '你好'] })
  const copy = await restarted.get('profile'); copy.enabled = false
  assert.equal((await store.get('profile')).enabled, true)
  assert.equal(await restarted.delete('profile'), true)
  assert.equal(await restarted.delete('profile'), false)
  assert.equal(await store.get('profile'), undefined)
  if (process.platform !== 'win32') assert.equal((await stat(join(a, 'storage.json'))).mode & 0o777, 0o600)
})

test('owner directories isolate equal keys and keys never become paths', async (t) => {
  const { a, b, store } = await fixture(t)
  const other = createStorage({ dataDir: b, isActive: () => true })
  await store.set('../outside', 'a')
  await other.set('../outside', 'b')
  await store.set('__proto__', { owner: 'a' })
  assert.equal(await store.get('../outside'), 'a')
  assert.equal(await other.get('../outside'), 'b')
  assert.deepEqual(await store.get('__proto__'), { owner: 'a' })
  assert.equal(await other.get('__proto__'), undefined)
  assert.equal(JSON.parse(await readFile(join(a, 'storage.json'), 'utf8')).entries['../outside'], 'a')
})

test('storage refuses invalid JSON, oversized values and keys without damaging state', async (t) => {
  const { a, store } = await fixture(t)
  await store.set('kept', 'original')
  const before = await readFile(join(a, 'storage.json'))
  const sparse = new Array(2)
  const cyclic = {}; cyclic.self = cyclic
  const symbol = { [Symbol('hidden')]: 1 }
  const extra = [1]; extra.custom = 2
  const getter = Object.defineProperty({}, 'x', { enumerable: true, get() { throw new Error('must not run') } })
  for (const value of [undefined, NaN, Infinity, 1n, new Date(), () => 1, sparse, cyclic, symbol, getter, extra]) {
    await assert.rejects(store.set('bad', value), code('invalid'))
  }
  for (const key of ['', 'nul\0key', '鲸'.repeat(STORAGE_LIMITS.keyBytes)]) await assert.rejects(store.set(key, 1), code('invalid'))
  await assert.rejects(store.set('big', 'x'.repeat(STORAGE_LIMITS.valueBytes)), code('limit'))
  assert.deepEqual(await readFile(join(a, 'storage.json')), before)
})

test('owner quota refuses the next write and allows replacement or deletion', async (t) => {
  const { store } = await fixture(t)
  const value = 'x'.repeat(STORAGE_LIMITS.valueBytes - 2)
  let count = 0
  for (;;) {
    try { await store.set(`key${count}`, value); count++ } catch (error) { assert.equal(error.code, 'limit'); break }
  }
  assert.ok(count > 0)
  assert.equal(await store.get('key0'), value)
  assert.equal(await store.get(`key${count}`), undefined)
  await store.set('key0', 'smaller')
  await store.set(`key${count}`, value)
  await store.delete('key1')
})

test('key-count quota is enforced when reading persisted state', async (t) => {
  const { a, store } = await fixture(t)
  const entries = Object.fromEntries(Array.from({ length: STORAGE_LIMITS.keys + 1 }, (_, i) => [`k${i}`, null]))
  const bytes = JSON.stringify({ version: 1, entries })
  await writeFile(join(a, 'storage.json'), bytes, { mode: 0o600 })
  await assert.rejects(store.get('k0'), code('corrupt'))
  await assert.rejects(store.set('x', 1), code('corrupt'))
  assert.equal(await readFile(join(a, 'storage.json'), 'utf8'), bytes)
  delete entries.k1024
  await writeFile(join(a, 'storage.json'), JSON.stringify({ version: 1, entries }))
  await assert.rejects(store.set('extra', 1), code('limit'))
  await store.set('k0', 1)
  await store.delete('k1')
  await store.set('extra', 2)
  assert.equal(await store.get('extra'), 2)
})

test('queued operations refuse a revoked owner and state survives revocation', async (t) => {
  const { a, store, revoke } = await fixture(t)
  await store.set('kept', 7)
  const pending = store.set('late', 8)
  revoke()
  await assert.rejects(pending, code('not_available'))
  await assert.rejects(store.get('kept'), code('not_available'))
  await assert.rejects(store.delete('kept'), code('not_available'))
  const restarted = createStorage({ dataDir: a, isActive: () => true })
  assert.equal(await restarted.get('kept'), 7)
  assert.equal(await restarted.get('late'), undefined)
})

test('a writer lease prevents conflicting or interrupted writes from losing state', async (t) => {
  const { a, store } = await fixture(t)
  await store.set('kept', 7)
  const bytes = await readFile(join(a, 'storage.json'))
  await writeFile(join(a, 'storage.lock'), 'another or interrupted writer', { flag: 'wx', mode: 0o600 })
  assert.equal(await store.get('kept'), 7)
  await assert.rejects(store.set('kept', 8), code('busy'))
  await assert.rejects(store.delete('kept'), code('busy'))
  assert.deepEqual(await readFile(join(a, 'storage.json')), bytes)
})

test('corrupt state fails closed and never gets replaced by an empty store', async (t) => {
  const { a, store } = await fixture(t)
  for (const bytes of ['{broken', '{"version":2,"entries":{}}', '{"version":1,"entries":[],"extra":true}']) {
    await writeFile(join(a, 'storage.json'), bytes)
    await assert.rejects(store.get('key'), code('corrupt'))
    await assert.rejects(store.set('key', 1), code('corrupt'))
    assert.equal(await readFile(join(a, 'storage.json'), 'utf8'), bytes)
  }
})

test('storage refuses a linked state file instead of reading or changing its target', { skip: process.platform === 'win32' }, async (t) => {
  const { a, b, store } = await fixture(t)
  const outside = join(b, 'untouched.json')
  const bytes = '{"version":1,"entries":{"secret":"untouched"}}'
  await writeFile(outside, bytes)
  await symlink(outside, join(a, 'storage.json'))
  await assert.rejects(store.get('secret'), code('corrupt'))
  await assert.rejects(store.set('secret', 'changed'), code('corrupt'))
  assert.equal(await readFile(outside, 'utf8'), bytes)
})

test('concurrent calls on one owner serialize without losing independent keys', async (t) => {
  const { store } = await fixture(t)
  await Promise.all(Array.from({ length: 24 }, (_, i) => store.set(`k${i}`, i)))
  assert.deepEqual(await Promise.all(Array.from({ length: 24 }, (_, i) => store.get(`k${i}`))), Array.from({ length: 24 }, (_, i) => i))
})

test('set snapshots input at invocation before callers can mutate it', async (t) => {
  const { store } = await fixture(t)
  const value = { counter: 1 }
  const pending = store.set('value', value)
  value.counter = 99
  await pending
  assert.deepEqual(await store.get('value'), { counter: 1 })
})

test('storage requires an assigned absolute directory and refuses directory links', { skip: process.platform === 'win32' }, async (t) => {
  const { a, b } = await fixture(t)
  assert.throws(() => createStorage({ dataDir: 'relative', isActive: () => true }), code('invalid'))
  const linked = join(a, 'linked')
  await symlink(b, linked)
  const store = createStorage({ dataDir: linked, isActive: () => true })
  await assert.rejects(store.set('key', 1), code('invalid'))
})
