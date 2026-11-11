import { execFileSync } from 'node:child_process'
import { copyFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const backend = fileURLToPath(new URL('../../../', import.meta.url))
execFileSync('cargo', ['build', '-p', 'cgg-decide-wasm', '--target', 'wasm32-unknown-unknown', '--release'], {
  cwd: backend,
  stdio: 'inherit',
})
copyFileSync(
  new URL('../../../target/wasm32-unknown-unknown/release/cgg_decide_wasm.wasm', import.meta.url),
  new URL('../src/decide.wasm', import.meta.url),
)
