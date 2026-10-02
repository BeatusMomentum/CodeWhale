/** Owner-local plugin state, never session history or a credential service. */
import { constants } from 'node:fs'
import { lstat, open, rename, unlink } from 'node:fs/promises'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { isJson } from '../json.ts'
import type { Json } from '../protocol.ts'

export const STORAGE_LIMITS = Object.freeze({ keyBytes: 128, valueBytes: 128 * 1024, totalBytes: 4 * 1024 * 1024, keys: 1024 })

export interface PluginStorage {
  get(key: string): Promise<Json | undefined>
  set(key: string, value: Json): Promise<void>
  delete(key: string): Promise<boolean>
}

export interface StorageOptions {
  /** The exact directory Rust already assigned to this owner. */
  dataDir: string
  /** Must become false as soon as this owner starts disposal or is revoked. */
  isActive: () => boolean
}

export class StorageError extends Error {
  readonly code: 'not_available' | 'invalid' | 'limit' | 'corrupt' | 'busy' | 'io'
  constructor(code: StorageError['code'], message: string) {
    super(message)
    this.code = code
    this.name = 'StorageError'
  }
}

function checkKey(key: string): void {
  if (typeof key !== 'string' || !key.length || key.includes('\0') || Buffer.byteLength(key) > STORAGE_LIMITS.keyBytes) {
    throw new StorageError('invalid', `storage key must contain 1 to ${STORAGE_LIMITS.keyBytes} UTF-8 bytes and no NUL`)
  }
}

/** Reject accessors, symbols, sparse arrays and extra properties before isJson reads values. */
function plainJson(value: unknown, depth = 0): boolean {
  if (depth > 64) return false
  if (value === null || typeof value !== 'object') return true
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return false
  const keys = Reflect.ownKeys(value)
  if (Array.isArray(value) && keys.length !== value.length + 1) return false
  for (const key of keys) {
    if (Array.isArray(value) && key === 'length') continue
    if (typeof key !== 'string') return false
    if (Array.isArray(value) && (!/^(0|[1-9]\d*)$/u.test(key) || Number(key) >= value.length)) return false
    const descriptor = Object.getOwnPropertyDescriptor(value, key)!
    if (!descriptor.enumerable || !('value' in descriptor) || !plainJson(descriptor.value, depth + 1)) return false
  }
  return true
}

function snapshot(value: unknown): Json {
  if (!plainJson(value) || !isJson(value)) throw new StorageError('invalid', 'storage value must be plain JSON')
  const encoded = JSON.stringify(value)
  if (Buffer.byteLength(encoded) > STORAGE_LIMITS.valueBytes) throw new StorageError('limit', 'storage value exceeds its byte limit')
  return JSON.parse(encoded)
}

function encode(entries: Record<string, Json>): string {
  if (Object.keys(entries).length > STORAGE_LIMITS.keys) throw new StorageError('limit', 'storage exceeds its key limit')
  const encoded = JSON.stringify({ version: 1, entries }) + '\n'
  if (Buffer.byteLength(encoded) > STORAGE_LIMITS.totalBytes) throw new StorageError('limit', 'storage exceeds its owner byte limit')
  return encoded
}

function fsCode(error: unknown): string | undefined {
  return (error as NodeJS.ErrnoException)?.code
}

/**
 * One API per owner generation. Keep isActive live; do not capture its initial value.
 * Calls queue within this API. An exclusive writer file also prevents lost updates
 * between host processes sharing this dataDir. An interrupted writer leaves a
 * visible, fail-closed busy error; this module never guesses whether to remove it.
 * A write admitted at the final active check may finish its atomic rename while
 * disposal runs. Later or queued calls are refused. Unload never deletes state.
 */
export function createStorage({ dataDir, isActive }: StorageOptions): PluginStorage {
  const path = join(dataDir, 'storage.json')
  const lockPath = join(dataDir, 'storage.lock')
  let tail: Promise<unknown> = Promise.resolve()

  function active(): void {
    if (!isActive()) throw new StorageError('not_available', 'plugin storage owner is no longer active')
  }

  async function directory(): Promise<void> {
    const stat = await lstat(dataDir)
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new StorageError('invalid', 'plugin storage dataDir must be a real directory')
    active()
  }

  async function load(): Promise<Record<string, Json>> {
    let file
    try {
      const before = await lstat(path)
      if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1) throw new StorageError('corrupt', 'plugin storage is not a single-link regular file')
      file = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0))
      const stat = await file.stat()
      if (!stat.isFile() || stat.nlink !== 1 || stat.dev !== before.dev || stat.ino !== before.ino) throw new StorageError('corrupt', 'plugin storage file changed while opening')
      if (stat.size > STORAGE_LIMITS.totalBytes) throw new StorageError('corrupt', 'plugin storage file exceeds its owner byte limit')
      const bytes = Buffer.alloc(Math.min(stat.size + 1, STORAGE_LIMITS.totalBytes + 1))
      let length = 0
      while (length < bytes.length) {
        const read = await file.read(bytes, length, bytes.length - length, length)
        if (!read.bytesRead) break
        length += read.bytesRead
      }
      if (length > stat.size) throw new StorageError('corrupt', 'plugin storage file changed while reading')
      const stored = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, length)))
      if (!stored || stored.version !== 1 || Object.keys(stored).length !== 2 || !stored.entries || Array.isArray(stored.entries) || typeof stored.entries !== 'object') {
        throw new StorageError('corrupt', 'plugin storage format is invalid')
      }
      for (const [key, value] of Object.entries(stored.entries)) {
        checkKey(key)
        snapshot(value)
      }
      encode(stored.entries)
      return stored.entries
    } catch (error) {
      if (fsCode(error) === 'ENOENT') return Object.create(null)
      if (error instanceof StorageError) throw new StorageError('corrupt', error.message)
      if (error instanceof SyntaxError || error instanceof TypeError) throw new StorageError('corrupt', 'plugin storage contents are invalid; existing state was preserved')
      throw error
    } finally {
      await file?.close()
    }
  }

  async function write(entries: Record<string, Json>): Promise<void> {
    const encoded = encode(entries)
    const temporary = join(dataDir, `.storage-${randomUUID()}.tmp`)
    let file
    try {
      file = await open(temporary, 'wx', 0o600)
      await file.writeFile(encoded)
      await file.sync()
      await file.close()
      file = undefined
      active()
      await rename(temporary, path)
      // The file and directory entry both reach durability before success.
      if (process.platform !== 'win32') {
        const dir = await open(dataDir, constants.O_RDONLY)
        try { await dir.sync() } finally { await dir.close() }
      }
    } finally {
      await file?.close()
      await unlink(temporary).catch((error) => { if (fsCode(error) !== 'ENOENT') throw error })
    }
  }

  async function mutate<T>(change: (entries: Record<string, Json>) => { entries: Record<string, Json>; result: T }): Promise<T> {
    let lock
    try {
      try { lock = await open(lockPath, 'wx', 0o600) } catch (error) {
        if (fsCode(error) === 'EEXIST') throw new StorageError('busy', 'plugin storage has another or interrupted writer (storage.lock); existing state was preserved')
        throw error
      }
      const identity = await lock.stat()
      try {
        active()
        const changed = change(await load())
        await write(changed.entries)
        return changed.result
      } finally {
        await lock.close()
        lock = undefined
        const current = await lstat(lockPath).catch((error) => { if (fsCode(error) !== 'ENOENT') throw error; return undefined })
        if (current?.dev === identity.dev && current.ino === identity.ino) await unlink(lockPath)
      }
    } finally {
      await lock?.close()
    }
  }

  function queue<T>(operation: () => Promise<T>): Promise<T> {
    const next = tail.then(async () => {
      active()
      await directory()
      try { return await operation() } catch (error) {
        if (error instanceof StorageError) throw error
        throw new StorageError('io', `plugin storage operation failed (${fsCode(error) ?? 'unknown filesystem error'})`)
      }
    })
    tail = next.catch(() => undefined)
    return next
  }

  return Object.freeze({
    get(key: string) {
      return queue(async () => { checkKey(key); const entries = await load(); active(); return Object.hasOwn(entries, key) ? entries[key] : undefined })
    },
    set(key: string, value: Json) {
      return queue(async () => { checkKey(key); const copy = snapshot(value); await mutate((entries) => ({ entries: { ...entries, [key]: copy }, result: undefined })) })
    },
    delete(key: string) {
      return queue(async () => {
        checkKey(key)
        return mutate((entries) => { const present = Object.hasOwn(entries, key); delete entries[key]; return { entries, result: present } })
      })
    },
  })
}
