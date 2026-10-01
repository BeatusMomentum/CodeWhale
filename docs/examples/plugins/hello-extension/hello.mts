// Node strips these erasable types; no compiler or package installation is needed.
type Input = { name?: string }
type Execution = { signal: AbortSignal; callId: string }
type Invocation = { args: string; signal: AbortSignal }

export const name = 'hello-extension'
export const inject = ['tools', 'commands']

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

  // `/hello-greet [name]`: a slash command the *user* runs. It can only
  // answer: text shown in the transcript, or `{ kind: 'submit', prompt }` to
  // send a prompt as the user's next message (the model's work after that
  // goes through the normal turn and its approvals).
  ctx.commands.register({
    name: 'hello-greet',
    description: 'Greet someone. This example does not read files or use the network.',
    argumentHint: '[name]',
    handler(invocation: Invocation) {
      invocation.signal.throwIfAborted()
      return { kind: 'success', text: `Hello, ${invocation.args || 'world'}!` }
    },
  })
}
