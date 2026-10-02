// Node strips these erasable types; no compiler or package installation is needed.
type Input = { name?: string }
type Execution = { signal: AbortSignal; callId: string }

export const name = 'hello-extension'
export const inject = ['tools']

export function apply(ctx: any) {
  ctx.tools.register({
    name: 'hello_greet',
    description: 'Return a greeting. This example does not read files or use the network.',
    parameters: {
      type: 'object',
      properties: { name: { type: 'string' } },
      additionalProperties: false,
    },
    execute(input: Input, exec: Execution) {
      exec.signal.throwIfAborted()
      return { greeting: `Hello, ${input.name ?? 'world'}!`, callId: exec.callId }
    },
  })
}
