import { defineConfig, devices } from '@playwright/test';

const PREVIEW_PORT = 4173;

export default defineConfig({
    testDir: './e2e',
    forbidOnly: true,
    retries: process.env.CI ? 2 : 0,
    reporter: 'list',
    webServer: {
        command: `npx vite build && npx vite preview --port ${String(PREVIEW_PORT)} --strictPort`,
        url: `http://localhost:${String(PREVIEW_PORT)}/`,
        reuseExistingServer: false,
    },
    projects: [
        {
            name: 'chromium',
            use: { ...devices['Desktop Chrome'], baseURL: `http://localhost:${String(PREVIEW_PORT)}/` },
        },
    ],
});
