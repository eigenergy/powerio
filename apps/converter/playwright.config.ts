import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: './tests',
  timeout: 120_000,
  expect: { timeout: 45_000 },
  // The hosted Firefox runner is several times slower to load the WebAssembly
  // engine than Chromium; one retry with the trace kept separates a slow
  // runner from a real failure.
  retries: process.env.CI ? 1 : 0,
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: { actionTimeout: 15_000, baseURL: 'http://127.0.0.1:4280/convert/', trace: 'retain-on-failure' },
  webServer: { command: 'npm run preview -- --port 4280', url: 'http://127.0.0.1:4280/convert/', reuseExistingServer: !process.env.CI },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'], launchOptions: process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {} } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'], actionTimeout: 45_000 } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
});
