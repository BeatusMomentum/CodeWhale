import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const bundle = readFileSync(join(here, '..', 'dist', 'builtin', 'mcp.mjs'), 'utf8')

test('bundled MCP OAuth errors omit response bodies and dynamic error details', () => {
  assert.doesNotMatch(bundle, /Raw body: \$\{body\}/)
  assert.doesNotMatch(bundle, /Cause: \$\{JSON\.stringify\(error2\.message\)\}/)
  assert.doesNotMatch(bundle, /Cause: \$\{JSON\.stringify\(error2 instanceof Error/)
  assert.match(bundle, /OAuth error response details omitted/)
  assert.ok(bundle.includes('falling back to a new authorization request.'))
})
