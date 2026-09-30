/**
 * Which JavaScript runtime this host runs on, and the native-code policy.
 *
 * The host runs on Bun (preferred) or Node. Bun emulates `process.versions.node`,
 * so the runtime is read from `process.versions.bun` first: `host/hello` must
 * report what is actually running.
 */
import { createRequire } from 'node:module'

export interface RuntimeInfo {
  name: 'bun' | 'node'
  version: string
}

export const RUNTIME: RuntimeInfo =
  typeof process.versions.bun === 'string'
    ? { name: 'bun', version: process.versions.bun }
    : { name: 'node', version: process.versions.node }

function refuse(what: string): never {
  throw new Error(`${what} is not available to extensions`)
}

/**
 * Remove native-code entry points before any plugin loads. The OS sandbox
 * (Seatbelt on macOS) stays the boundary; this is defense in depth.
 *
 * - `process.dlopen` loads a shared library in-process on both runtimes.
 *   Node also runs with `--no-addons`.
 * - Under Bun, `bun:ffi` calls C directly and `--no-addons` does not cover it.
 *   Bun will not let `Bun.plugin` replace a builtin module. But the builtin's
 *   export object is shared by `import` and `require`, and it can be changed.
 *   Every export becomes a getter that throws, and the object is frozen. So
 *   `import { dlopen } from 'bun:ffi'` fails at import, and so do
 *   `require('bun:ffi')`, `CString`, `read.*` and `toArrayBuffer`.
 */
export function denyNativeCode() {
  Object.defineProperty(process, 'dlopen', {
    value: () => refuse('process.dlopen'),
    writable: false,
    configurable: false,
  })
  if (RUNTIME.name !== 'bun') return
  const ffi = createRequire(import.meta.url)('bun:ffi') as Record<string, unknown>
  for (const name of Object.keys(ffi)) {
    Object.defineProperty(ffi, name, {
      get: () => refuse('`bun:ffi`'),
      enumerable: true,
      configurable: false,
    })
  }
  Object.freeze(ffi)
}
