import { defineConfig, devices } from '@playwright/test';

const ORIGIN = 'http://127.0.0.1:8181';

// The real server hosts the built app and runs the engine stub. e2e/serve.ts builds everything first, so the first start
// takes a while.
export default defineConfig({
  testDir: './e2e',
  forbidOnly: true,
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 2 : 0,
  reporter: 'list',
  webServer: {
    command: 'node e2e/serve.ts',
    url: `${ORIGIN}/api/info`,
    reuseExistingServer: false,
    timeout: 600_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
  expect: { timeout: 15_000 },
  use: { baseURL: ORIGIN },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
