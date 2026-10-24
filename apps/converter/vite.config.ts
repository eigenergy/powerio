import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { execFileSync } from 'node:child_process';

export default defineConfig({
  base: '/convert/',
  plugins: [svelte()],
  define: { __BUILD_COMMIT__: JSON.stringify(execFileSync('git', ['rev-parse', '--short', 'HEAD']).toString().trim()) },
  build: { target: 'es2022', assetsInlineLimit: 0 },
  worker: { format: 'es' },
  test: { include: ['src/**/*.test.ts'] },
});
