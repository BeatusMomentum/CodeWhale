// @generated from the Rust protocol types in crates/tui/src/extension_host/protocol.rs
// by `extension_host::protocol::tests`. Do not edit: change the Rust side, re-record with
//   CODEWHALE_CONFORMANCE_UPDATE=1 cargo test -p codewhale-tui --lib extension_host::protocol
// and rebuild dist/ with `npm run build`.

export const PROTOCOL_VERSION = 1
export const MAGIC_ASCII = 'CWX1'
export const HEADER_LEN = 8
export const MAX_FRAME = 33554432
export const MAX_INFLIGHT = 256

/** JSON-RPC error codes used on this channel. */
export const ErrorCode = {
  ParseError: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  Internal: -32603,
  ExecutionFailed: -32000,
  NotAvailable: -32001,
  Refused: -32002,
  Denied: -32003,
  Cancelled: -32800,
} as const

export type Direction = 'core_to_host' | 'host_to_core'

/** Every method either side may send, and the trust tiers it is allowed on; nothing else is admitted. */
export const METHODS = [
  { name: 'host/initialize', direction: 'core_to_host', request: true, params: 'InitializeParams', tiers: ['plugin', 'builtin'] },
  { name: 'host/ping', direction: 'core_to_host', request: true, params: 'EmptyParams', tiers: ['plugin', 'builtin'] },
  { name: 'host/shutdown', direction: 'core_to_host', request: true, params: 'EmptyParams', tiers: ['plugin', 'builtin'] },
  { name: 'ext/activate', direction: 'core_to_host', request: true, params: 'ActivateParams', tiers: ['plugin', 'builtin'] },
  { name: 'ext/deactivate', direction: 'core_to_host', request: true, params: 'DeactivateParams', tiers: ['plugin', 'builtin'] },
  { name: 'tool/call', direction: 'core_to_host', request: true, params: 'ToolCallParams', tiers: ['plugin', 'builtin'] },
  { name: 'command/run', direction: 'core_to_host', request: true, params: 'CommandRunParams', tiers: ['plugin', 'builtin'] },
  { name: 'hook/evaluate', direction: 'core_to_host', request: true, params: 'HookEvaluateParams', tiers: ['plugin', 'builtin'] },
  { name: '$/cancel', direction: 'core_to_host', request: false, params: 'CancelParams', tiers: ['plugin', 'builtin'] },
  { name: 'host/hello', direction: 'host_to_core', request: false, params: 'HelloParams', tiers: ['plugin', 'builtin'] },
  { name: 'host/ready', direction: 'host_to_core', request: false, params: 'EmptyParams', tiers: ['plugin', 'builtin'] },
  { name: 'registry/register', direction: 'host_to_core', request: true, params: 'RegisterParams', tiers: ['plugin', 'builtin'] },
  { name: 'registry/unregister', direction: 'host_to_core', request: true, params: 'UnregisterParams', tiers: ['plugin', 'builtin'] },
  { name: 'core/call', direction: 'host_to_core', request: true, params: 'CoreCallParams', tiers: ['plugin', 'builtin'] },
  { name: 'ext/faulted', direction: 'host_to_core', request: false, params: 'FaultedParams', tiers: ['plugin', 'builtin'] },
  { name: 'log', direction: 'host_to_core', request: false, params: 'LogParams', tiers: ['plugin', 'builtin'] },
  { name: '$/cancel', direction: 'host_to_core', request: false, params: 'CancelParams', tiers: ['plugin', 'builtin'] },
] as const

/** A field's wire kind: the Rust field's serde type, normalized for validation. */
export type Kind =
  | 'string'
  | 'boolean'
  | 'uint'
  | 'integer'
  | 'object'
  | 'json'
  | { readonly ref: string }
  | { readonly enum: readonly string[] }
  | { readonly items: Kind }

/** An object's fields; `strict` is Rust's `deny_unknown_fields`. */
export interface Shape {
  readonly strict: boolean
  readonly required: { readonly [field: string]: Kind }
  readonly optional: { readonly [field: string]: Kind }
}

/** Every method's params, and an error response's `error`. */
export const SHAPES: { readonly [name: string]: Shape } = {
  ActivateParams: {
    strict: false,
    required: { owner: { ref: 'OwnerRef' }, plugin_name: 'string', entry: { ref: 'EntryRef' } },
    optional: { config: 'json', data_dir: 'string' },
  },
  CancelParams: {
    strict: true,
    required: { id: 'uint' },
    optional: {},
  },
  CommandRunParams: {
    strict: false,
    required: { handle: 'uint', command_id: 'string', raw_input: 'string', deadline_ms: 'uint' },
    optional: { workspace: 'string', session_id: 'string', agent_id: 'string', origin_turn_id: 'string' },
  },
  CoreCallParams: {
    strict: true,
    required: { owner: { ref: 'OwnerRef' }, ticket: 'string', name: 'string', input: 'json' },
    optional: {},
  },
  DeactivateParams: {
    strict: false,
    required: { owner: { ref: 'OwnerRef' } },
    optional: {},
  },
  EmptyParams: {
    strict: true,
    required: {},
    optional: {},
  },
  EntryRef: {
    strict: false,
    required: { path: 'string', sha256: 'string' },
    optional: {},
  },
  FaultedParams: {
    strict: true,
    required: { owner: { ref: 'OwnerRef' }, error: 'string' },
    optional: {},
  },
  HelloParams: {
    strict: true,
    required: { protocol: { ref: 'ProtocolRange' }, host_version: 'string', bundle_sha256: 'string', runtime: { ref: 'HelloRuntime' }, tier: { enum: ['builtin', 'plugin'] }, builtin_modules: { items: { ref: 'ModuleDigestWire' } } },
    optional: { memory_limit_mib: 'uint' },
  },
  HelloRuntime: {
    strict: true,
    required: { name: 'string', version: 'string' },
    optional: {},
  },
  HookCallPayload: {
    strict: true,
    required: { name: 'string', call_id: 'string', input: 'json', mode: 'string', workspace: 'string', model: 'string' },
    optional: {},
  },
  HookEvaluateParams: {
    strict: true,
    required: { handle: 'uint', event: 'string', payload: { ref: 'HookCallPayload' }, deadline_ms: 'uint' },
    optional: {},
  },
  HostLimits: {
    strict: false,
    required: { max_frame: 'uint', max_inflight: 'uint', dispose_deadline_ms: 'uint', activate_deadline_ms: 'uint' },
    optional: {},
  },
  InitializeParams: {
    strict: false,
    required: { protocol: 'uint', limits: { ref: 'HostLimits' } },
    optional: {},
  },
  LogParams: {
    strict: true,
    required: { level: 'string', msg: 'string' },
    optional: { plugin_id: 'string' },
  },
  ModuleDigestWire: {
    strict: true,
    required: { id: 'string', sha256: 'string' },
    optional: {},
  },
  OwnerRef: {
    strict: true,
    required: { plugin_id: 'string', generation: 'uint', owner_token: 'string' },
    optional: {},
  },
  ProtocolRange: {
    strict: true,
    required: { min: 'uint', max: 'uint' },
    optional: {},
  },
  RegisterParams: {
    strict: true,
    required: { owner: { ref: 'OwnerRef' }, kind: { enum: ['tool', 'command', 'hook', 'prompt_section', 'skill_root'] }, spec: { ref: 'RegisterSpecWire' } },
    optional: {},
  },
  RegisterSpecWire: {
    strict: true,
    required: { name: 'string', description: 'string' },
    optional: { input_schema: 'object', argument_hint: 'string' },
  },
  RpcErrorWire: {
    strict: true,
    required: { code: 'integer', message: 'string' },
    optional: { data: 'json' },
  },
  ToolCallParams: {
    strict: false,
    required: { handle: 'uint', call_id: 'string', input: 'json', deadline_ms: 'uint' },
    optional: { workspace: 'string', ticket: 'string', session_id: 'string', agent_id: 'string', origin_turn_id: 'string' },
  },
  UnregisterParams: {
    strict: true,
    required: { owner: { ref: 'OwnerRef' }, handle: 'uint' },
    optional: {},
  },
}

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

export interface ActivateParams {
  owner: OwnerRef
  plugin_name: string
  entry: EntryRef
  config?: Json
  data_dir?: string
}

export type ActivateResult = { status: 'ok'; tools: string[]; commands?: string[] } | { status: 'failed'; diagnostic: string }

export interface CancelParams {
  id: number
}

export type CommandResultWire = { kind: 'success'; text?: string } | { kind: 'error'; text: string } | { kind: 'submit'; prompt: string; text?: string }

export interface CommandRunParams {
  handle: number
  command_id: string
  raw_input: string
  deadline_ms: number
  workspace?: string
  session_id?: string
  agent_id?: string
  origin_turn_id?: string
}

export type ContentBlockWire = { type: 'text'; text: string }

export interface CoreCallParams {
  owner: OwnerRef
  ticket: string
  name: string
  input: Json
}

export interface DeactivateParams {
  owner: OwnerRef
}

export interface DeactivateResult {
  disposed: boolean
  leaked: string[]
}

export interface EmptyParams {}

export interface EntryRef {
  path: string
  sha256: string
}

export interface FaultedParams {
  owner: OwnerRef
  error: string
}

export interface HelloParams {
  protocol: ProtocolRange
  host_version: string
  bundle_sha256: string
  runtime: HelloRuntime
  tier: HostTier
  builtin_modules: ModuleDigestWire[]
  memory_limit_mib?: number
}

export interface HelloRuntime {
  name: string
  version: string
}

export interface HookCallPayload {
  name: string
  call_id: string
  input: Json
  mode: string
  workspace: string
  model: string
}

export interface HookEvaluateParams {
  handle: number
  event: string
  payload: HookCallPayload
  deadline_ms: number
}

export type HookVerdictWire = { kind: 'abstain' } | { kind: 'deny'; reason: string } | { kind: 'ask'; reason: string } | { kind: 'annotate'; text: string } | { kind: 'revise'; input: { [key: string]: Json } }

export interface HostLimits {
  max_frame: number
  max_inflight: number
  dispose_deadline_ms: number
  activate_deadline_ms: number
}

export type HostTier = 'builtin' | 'plugin'

export interface InitializeParams {
  protocol: number
  limits: HostLimits
}

export interface LogParams {
  level: string
  msg: string
  plugin_id?: string
}

export interface ModuleDigestWire {
  id: string
  sha256: string
}

export interface OwnerRef {
  plugin_id: string
  generation: number
  owner_token: string
}

export interface ProtocolRange {
  min: number
  max: number
}

export type RegisterKind = 'tool' | 'command' | 'hook' | 'prompt_section' | 'skill_root'

export interface RegisterParams {
  owner: OwnerRef
  kind: RegisterKind
  spec: RegisterSpecWire
}

export type RegisterResult = { handle: number } | { refused: string }

export interface RegisterSpecWire {
  name: string
  description: string
  input_schema?: { [key: string]: Json }
  argument_hint?: string
}

export interface RpcErrorWire {
  code: number
  message: string
  data?: Json
}

export interface ToolCallParams {
  handle: number
  call_id: string
  input: Json
  deadline_ms: number
  workspace?: string
  ticket?: string
  session_id?: string
  agent_id?: string
  origin_turn_id?: string
}

export interface ToolResultWire {
  content: ContentBlockWire[]
  is_error: boolean
  structured?: Json
}

export interface UnregisterParams {
  owner: OwnerRef
  handle: number
}
