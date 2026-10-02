/**
 * Which trust tier this host process serves.
 *
 * The core starts one host process per tier from the same bundle and says
 * which with `--tier=plugin|builtin`. A plugin-tier host runs reviewed
 * third-party plugins, whose owner ids are the plugin ids the core's discovery
 * builds (`<scope>/<hex>/<name>`); a builtin-tier host runs Codewhale's own
 * host code, whose owner ids are `host:<module>`. The two id spaces cannot
 * meet, and each process refuses an owner of the other tier
 * (`HostRoot.activate`), so a core that mixed them up would be told so
 * instead of running one tier's code in the other's process.
 *
 * Not a security boundary on its own (the OS sandbox and the separate
 * processes are); the Rust core is the authority on what runs where.
 */
export type HostTier = 'plugin' | 'builtin'

/** Every tier-0 owner id starts with this. Mirrors `tier::HOST_OWNER_PREFIX` in Rust. */
export const HOST_OWNER_PREFIX = 'host:'

const TIERS: readonly string[] = ['plugin', 'builtin']

/**
 * The tier named by `--tier=` in `argv` (the arguments after the script), or
 * `plugin` when there is none: the least-privileged tier is the default for a
 * host started by hand. An unknown value, a flag without a value, or a tier
 * named twice throws, so the host refuses to start rather than guess.
 */
export function parseTier(argv: readonly string[]): HostTier {
  let found: HostTier | undefined
  for (const arg of argv) {
    if (arg !== '--tier' && !arg.startsWith('--tier=')) continue
    const value = arg.startsWith('--tier=') ? arg.slice('--tier='.length) : ''
    if (!TIERS.includes(value)) {
      throw new Error(`unknown host tier ${JSON.stringify(value)} (expected --tier=plugin or --tier=builtin)`)
    }
    if (found !== undefined) throw new Error('the host tier was given more than once')
    found = value as HostTier
  }
  return found ?? 'plugin'
}

/** The tier an owner id belongs to. Total: the id decides. */
export function ownerTier(ownerId: string): HostTier {
  return ownerId.startsWith(HOST_OWNER_PREFIX) ? 'builtin' : 'plugin'
}
