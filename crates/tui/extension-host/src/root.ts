/**
 * The Cordis root that plugin fibers run under.
 *
 * The host has no turn loop, store, prompt authority or approval. Plugins
 * reach the core only through the shim services here, and every one of them
 * becomes a `registry/*` request that Rust admits or refuses. Service names
 * that belong to the core cannot be provided by a plugin at all.
 */
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { AsyncLocalStorage } from 'node:async_hooks'
import { pathToFileURL } from 'node:url'
import { Context, Inject, Service } from '@deepseek-ai/cordis'
import {
  ErrorCode,
  type ActivateParams,
  type ActivateResult,
  type CommandResultWire,
  type ContentBlockWire,
  type DeactivateResult,
  type Json,
  type OwnerRef,
  type ToolResultWire,
} from './protocol.ts'
import { RpcError, type RpcPeer } from './rpc.ts'
import { explainImportError } from './dsh/resolve-hooks.ts'
import { OwnedRegistrations } from './shims/owned.ts'
import { ownerTier, type HostTier } from './tier.ts'
import {
  commandSpec,
  defineCommandsService,
  makeInvocation,
  normalizeResult,
  type LocalCommand,
  type NormalizedCommand,
} from './shims/commands.ts'

/** Context key carrying the owner record; inherited by every nested fiber. */
export const OWNER = Symbol.for('codewhale.extension-host.owner')

/**
 * Service names a plugin may never provide: each is one authority the Rust
 * core owns (§4.3 of the design). `tools`, `commands` and `logger` are
 * provided by the host root as shims and are refused to plugins for the same
 * reason.
 */
export const REFUSED_SERVICES = new Set([
  'approval',
  'agents',
  'sessions',
  'llm',
  'sandboxPolicy',
  'credentials',
  'fs',
  'subprocess',
  'systemPrompt',
  'tools',
  'commands',
  'skills',
  'logger',
])

/** Services the root provides; `inject` of anything else fails activation. */
const PROVIDED_SERVICES = new Set(['tools', 'commands', 'logger', 'events', 'reflect', 'registry'])

const ACTIVATE_DEADLINE_MS = 5_000
const DISPOSE_DEADLINE_MS = 2_000

export interface OwnerRecord {
  ref: OwnerRef
  pluginName: string
  fibers: any[]
  /** In-flight `registry/register` requests, awaited before activation acks. */
  pendingRegistrations: Set<Promise<void>>
  refusals: string[]
  tools: Map<number, LocalTool>
  commands: Map<number, LocalCommand<OwnerRecord>>
  /** Entry modules activated under this owner so far (a plugin may declare several). */
  entries: Set<string>
  /** The plugin's own writable directory, as the core named it at activation. */
  dataDir?: string
  disposing?: Promise<void>
  state: 'activating' | 'active' | 'failed' | 'disposed'
}

interface LocalTool {
  owner: OwnerRecord
  name: string
  handle?: number
  definition: any
  disposed: boolean
}

export const ownerStorage = new AsyncLocalStorage<OwnerRecord>()

function describeError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`
  return String(error)
}

function withDeadline<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} exceeded ${ms} ms`)), ms)
    timer.unref()
  })
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer))
}

function isJson(value: unknown, depth = 0): value is Json {
  if (depth > 64) return false
  if (value === null) return true
  switch (typeof value) {
    case 'boolean':
    case 'string':
      return true
    case 'number':
      return Number.isFinite(value)
    case 'object':
      if (Array.isArray(value)) return value.every((v) => isJson(v, depth + 1))
      if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return false
      return Object.values(value as object).every((v) => isJson(v, depth + 1))
    default:
      return false
  }
}

export class HostRoot {
  readonly root: any
  readonly owners = new Map<string, OwnerRecord>()
  private readonly toolRegistrations: OwnedRegistrations<OwnerRecord, LocalTool>
  private readonly commandRegistrations: OwnedRegistrations<OwnerRecord, LocalCommand<OwnerRecord>>

  constructor(
    private readonly rpc: RpcPeer,
    /** The trust tier this process serves; it activates only owners of that tier. */
    readonly tier: HostTier,
  ) {
    const root: any = new Context()
    this.root = root
    const host = this
    this.toolRegistrations = new OwnedRegistrations(
      rpc,
      'tool',
      (owner) => owner.tools,
      (message, owner) => this.log('warn', message, owner),
    )
    this.commandRegistrations = new OwnedRegistrations(
      rpc,
      'command',
      (owner) => owner.commands,
      (message, owner) => this.log('warn', message, owner),
    )

    // Refuse core service names before any plugin can run. The refusal does
    // not depend on who calls: `ctx.root.provide(...)` runs with the root as
    // its context, so an owner check alone could be sidestepped. The one
    // exception is each host shim of its own name, provided once, below.
    const shimClasses = new Map<string, Function>()
    const shimsProvided = new Set<string>()
    const reflect = root.reflect
    const originalProvide = reflect.provide
    reflect.provide = function (this: any, name: string, value: unknown, ...rest: unknown[]) {
      if (REFUSED_SERVICES.has(name)) {
        const shim = shimClasses.get(name)
        const hostShim = shim !== undefined && !shimsProvided.has(name) && value instanceof (shim as any)
        if (!hostShim) {
          throw new Error(`extension may not provide core service \`${name}\`: the Codewhale core owns it`)
        }
        shimsProvided.add(name)
      }
      return originalProvide.call(this, name, value, ...rest)
    }

    // Logger shim: every Cordis log line becomes a `log` notification.
    root.logger.exporter({
      colors: false,
      export: (message: any) => {
        const fiber = message.fiber?.deref?.()
        const owner: OwnerRecord | undefined = fiber?.ctx?.[OWNER]
        const text = (message.args ?? []).map((arg: unknown) => (arg instanceof Error ? describeError(arg) : typeof arg === 'string' ? arg : safeStringify(arg))).join(' ')
        host.log(message.type === 'error' ? 'error' : message.type === 'warn' ? 'warn' : 'info', `[${message.name}] ${text}`, owner)
      },
    })

    class ToolsShim extends Service {
      constructor(ctx: any) {
        super(ctx, 'tools')
      }

      /** DSH `ctx.tools.register(defineTool(...))`: returns an idempotent disposer. */
      register(definition: any) {
        const ctx: any = this.ctx
        const owner: OwnerRecord | undefined = ctx[OWNER]
        if (!owner) throw new Error('tools.register called outside an extension owner')
        validateDefinition(definition)
        return ctx.effect(() => host.addTool(owner, definition), `tools.register(${JSON.stringify(definition.name)})`)
      }
    }
    // Plugins share one process, so the owner token is not a boundary
    // between them (design §4.4, threat 3). Freezing a shim at least stops
    // the direct route of one plugin rewriting `register` for every other
    // plugin; shared globals remain, and the approval card says so.
    Object.freeze(ToolsShim.prototype)
    const CommandsShim = defineCommandsService<OwnerRecord>({
      ownerOf: (ctx) => ctx[OWNER],
      addCommand: (owner, command) => host.addCommand(owner, command),
    })
    shimClasses.set('tools', ToolsShim)
    shimClasses.set('commands', CommandsShim)
    root.plugin(ToolsShim)
    root.plugin(CommandsShim)
  }

  log(level: string, msg: string, owner?: OwnerRecord) {
    const params: Record<string, string> = { level, msg: msg.slice(0, 8192) }
    if (owner) params.plugin_id = owner.ref.plugin_id
    this.rpc.notify('log', params)
  }

  /** Called inside the owner's effect; returns the effect's cleanup. */
  private addTool(owner: OwnerRecord, definition: any): () => void {
    const local: LocalTool = { owner, name: definition.name, definition, disposed: false }
    return this.toolRegistrations.add(local, {
      name: definition.name,
      description: String(definition.description ?? ''),
      input_schema: definition.parameters ?? { type: 'object', properties: {} },
    })
  }

  private addCommand(owner: OwnerRecord, definition: NormalizedCommand): () => void {
    const local: LocalCommand<OwnerRecord> = { owner, name: definition.name, definition, disposed: false }
    return this.commandRegistrations.add(local, commandSpec(definition))
  }

  /**
   * `ext/activate`: load one `native` entry of a plugin under its owner. A
   * manifest may declare several entries; the core sends one `ext/activate`
   * per entry, in order, under the same owner token, and every entry becomes a
   * fiber of that one owner. A further entry is accepted only after the
   * previous one finished activating, for the same plugin, and only once per
   * path. Any failure fails the whole owner: its fibers, the earlier entries'
   * included, are disposed and the owner forgotten (all-or-nothing).
   */
  async activate(params: ActivateParams): Promise<ActivateResult> {
    // This process serves one tier. An owner of the other tier is refused
    // outright, before anything of it is read or loaded.
    const wanted = ownerTier(params.owner.plugin_id)
    if (wanted !== this.tier) {
      throw new RpcError(
        ErrorCode.InvalidParams,
        `owner ${JSON.stringify(params.owner.plugin_id)} belongs to the ${wanted} tier, but this host serves the ${this.tier} tier`,
      )
    }
    const key = params.owner.owner_token
    const existing = this.owners.get(key)
    if (existing) {
      if (existing.state !== 'active') return { status: 'failed', diagnostic: 'owner token already active' }
      if (existing.ref.plugin_id !== params.owner.plugin_id || existing.pluginName !== params.plugin_name) {
        return { status: 'failed', diagnostic: 'owner token already active for another plugin' }
      }
      if (existing.entries.has(params.entry.path)) {
        return { status: 'failed', diagnostic: `entry ${params.entry.path} is already activated under this owner` }
      }
      existing.state = 'activating'
    }
    const owner: OwnerRecord = existing ?? {
      ref: params.owner,
      pluginName: params.plugin_name,
      fibers: [],
      pendingRegistrations: new Set(),
      refusals: [],
      tools: new Map(),
      commands: new Map(),
      entries: new Set(),
      ...(params.data_dir === undefined ? {} : { dataDir: params.data_dir }),
      state: 'activating',
    }
    if (!existing) this.owners.set(key, owner)
    owner.entries.add(params.entry.path)
    try {
      const bytes = await readFile(params.entry.path)
      const digest = createHash('sha256').update(bytes).digest('hex')
      if (digest !== params.entry.sha256) {
        throw new Error(`entry ${params.entry.path} changed after review (sha256 ${digest.slice(0, 12)}…)`)
      }
      const module = await ownerStorage.run(owner, () => import(pathToFileURL(params.entry.path).href)).catch((error) => {
        throw explainImportError(error)
      })
      const plugin = pickPlugin(module)
      const missing = Object.keys(Inject.resolve(plugin.inject)).filter((name) => !PROVIDED_SERVICES.has(name))
      if (missing.length > 0) {
        throw new Error(`requires ${missing.map((name) => `\`${name}\``).join(', ')}, not provided by the Codewhale extension host in this phase`)
      }
      const ownerCtx = this.root.extend({ [OWNER]: owner })
      const fiber = ownerStorage.run(owner, () => ownerCtx.plugin(plugin, params.config ?? {}))
      owner.fibers.push(fiber)
      await withDeadline(Promise.resolve(fiber.await()), ACTIVATE_DEADLINE_MS, 'activation')
      const pending = pendingFibers(fiber)
      if (pending.length > 0) {
        throw new Error(`plugin fibers are waiting on services the host does not provide: ${pending.join(', ')}`)
      }
      while (owner.pendingRegistrations.size > 0) {
        await Promise.allSettled([...owner.pendingRegistrations])
      }
      if (owner.refusals.length > 0) throw new Error(owner.refusals.join('; '))
      owner.state = 'active'
      return {
        status: 'ok',
        tools: [...owner.tools.values()].map((tool) => tool.name).sort(),
        commands: [...owner.commands.values()].map((command) => command.name).sort(),
      }
    } catch (error) {
      owner.state = 'failed'
      // All-or-nothing: dispose the partial fiber, rolling back every registration.
      await this.disposeOwner(owner).catch(() => undefined)
      this.owners.delete(key)
      return { status: 'failed', diagnostic: describeError(error) }
    }
  }

  /** Dispose one owner's fibers (reverse order, async disposers awaited). Memoised. */
  disposeOwner(owner: OwnerRecord): Promise<void> {
    owner.disposing ??= (async () => {
      for (const fiber of [...owner.fibers].reverse()) {
        await fiber.dispose()
      }
      owner.state = 'disposed'
    })()
    return owner.disposing
  }

  async deactivate(ref: OwnerRef): Promise<DeactivateResult> {
    const owner = this.owners.get(ref.owner_token)
    if (!owner) return { disposed: true, leaked: [] }
    let disposed = true
    try {
      await withDeadline(this.disposeOwner(owner), DISPOSE_DEADLINE_MS, 'dispose')
    } catch {
      disposed = false
    }
    const leaked = [
      ...[...owner.tools.values()].map((tool) => `tool:${tool.name}`),
      ...[...owner.commands.values()].map((command) => `command:${command.name}`),
    ]
    for (const fiber of owner.fibers) {
      for (const effect of fiber.getEffects?.() ?? []) leaked.push(`effect:${effect.label}`)
    }
    this.toolRegistrations.forget(owner)
    this.commandRegistrations.forget(owner)
    this.owners.delete(ref.owner_token)
    return { disposed, leaked }
  }

  async deactivateAll(deadlineMs: number) {
    await withDeadline(
      Promise.allSettled([...this.owners.values()].map((owner) => this.deactivate(owner.ref))),
      deadlineMs,
      'shutdown',
    ).catch(() => undefined)
  }

  async callTool(handle: number, input: unknown, callId: string, signal: AbortSignal, workspace?: string): Promise<ToolResultWire> {
    const local = this.toolRegistrations.byHandle.get(handle)
    if (!local || local.disposed || local.owner.state !== 'active') {
      throw new RpcError(ErrorCode.NotAvailable, `tool handle ${handle} is not live`)
    }
    const definition = local.definition
    // `workspace` is the calling session's workspace, per call; `dataDir` is
    // this plugin's own directory. Both are read-only strings.
    const exec = Object.freeze({ signal, callId, args: input, ...callContext(local.owner, workspace) })
    const run = ownerStorage.run(local.owner, async () => {
      const value = await definition.execute(input, exec)
      return renderResult(definition, input, value)
    })
    return Promise.race([run, abortedBy(signal)])
  }

  /** `command/run`: only for a handle the core admitted, and only on a user's own invocation. */
  async callCommand(handle: number, rawInput: string, commandId: string, signal: AbortSignal, workspace?: string): Promise<CommandResultWire> {
    const local = this.commandRegistrations.byHandle.get(handle)
    if (!local || local.disposed || local.owner.state !== 'active') {
      throw new RpcError(ErrorCode.NotAvailable, `command handle ${handle} is not live`)
    }
    const { definition } = local
    const run = ownerStorage.run(local.owner, async () => {
      const value = await definition.handler(makeInvocation(rawInput, commandId, signal, callContext(local.owner, workspace)))
      return normalizeResult(definition.name, value)
    })
    return Promise.race([run, abortedBy(signal)])
  }
}

/** The read-only context a call carries beyond its input; a field the core did not send is absent. */
function callContext(owner: OwnerRecord, workspace: string | undefined): { workspace?: string; dataDir?: string } {
  return {
    ...(workspace === undefined ? {} : { workspace }),
    ...(owner.dataDir === undefined ? {} : { dataDir: owner.dataDir }),
  }
}

/** Rejects as cancelled when `signal` aborts. */
function abortedBy(signal: AbortSignal): Promise<never> {
  return new Promise<never>((_, reject) => {
    const onAbort = () => reject(new RpcError(ErrorCode.Cancelled, 'cancelled'))
    if (signal.aborted) onAbort()
    else signal.addEventListener('abort', onAbort, { once: true })
  })
}

function renderResult(definition: any, input: unknown, value: unknown): ToolResultWire {
  let blocks: unknown = undefined
  if (typeof definition.output?.render === 'function') {
    blocks = definition.output.render(input, value)
  }
  const content: ContentBlockWire[] = []
  if (Array.isArray(blocks)) {
    for (const block of blocks) {
      if (block && typeof block === 'object' && (block as any).type === 'text' && typeof (block as any).text === 'string') {
        content.push({ type: 'text', text: (block as any).text })
      }
    }
  } else {
    content.push({ type: 'text', text: typeof value === 'string' ? value : safeStringify(value) })
  }
  const result: ToolResultWire = { content, is_error: false }
  if (value !== undefined && isJson(value)) result.structured = value
  return result
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return String(value)
  }
}

function validateDefinition(definition: any) {
  if (!definition || typeof definition !== 'object') throw new TypeError('tool definition must be an object')
  if (typeof definition.name !== 'string' || definition.name.length === 0) throw new TypeError('tool definition needs a name')
  if (typeof definition.execute !== 'function') throw new TypeError(`tool \`${definition.name}\` needs an execute function`)
  if (definition.parameters !== undefined && (typeof definition.parameters !== 'object' || definition.parameters === null)) {
    throw new TypeError(`tool \`${definition.name}\` parameters must be a JSON schema object`)
  }
}

function pickPlugin(module: any): any {
  const candidate = module?.default ?? module
  if (typeof candidate === 'function') return candidate
  if (candidate && typeof candidate.apply === 'function') return candidate
  if (module && typeof module.apply === 'function') return module
  throw new Error('entry module exports no Cordis plugin (a function, or an object with `apply`)')
}

/** Names of services that nested fibers under `fiber` are still waiting on. */
function pendingFibers(fiber: any): string[] {
  const out: string[] = []
  const seen = new Set<any>()
  const visit = (current: any) => {
    if (!current || seen.has(current)) return
    seen.add(current)
    if (current.state === 0 /* PENDING */) {
      const names = Object.keys(current.inject ?? {})
      out.push(`${current.name ?? 'plugin'} (${names.join(', ')})`)
    }
  }
  visit(fiber)
  for (const runtime of fiber.ctx?.registry?.values?.() ?? []) {
    for (const child of runtime.fibers ?? []) {
      if (child.parent?.[OWNER] === fiber.ctx?.[OWNER]) visit(child)
    }
  }
  return out
}
