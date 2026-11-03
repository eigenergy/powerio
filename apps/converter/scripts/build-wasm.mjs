import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const crate = fileURLToPath(new URL('../../../powerio-wasm', import.meta.url));
const out = fileURLToPath(new URL('../src/wasm', import.meta.url));
const result = spawnSync('wasm-pack', ['build', crate, '--target', 'web', '--out-dir', out, '--release', '--locked'], {
  stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0' },
});
process.exit(result.status ?? 1);
