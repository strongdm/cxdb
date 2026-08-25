// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { defineConfig, devices } from '@playwright/test';

// When using external servers (CXDB_TEST_ADDR is set), skip local server management
const useExternalServer = !!process.env.CXDB_TEST_ADDR;
const frontendPort = Number(process.env.CXDB_TEST_FRONTEND_PORT ?? '3000');
const frontendURL = `http://127.0.0.1:${frontendPort}`;

/**
 * Playwright configuration for CXDB integration tests.
 *
 * These tests validate the full stack: Rust service, Go writer, and React frontend.
 * Tests run sequentially (workers: 1) because each test needs its own server instance.
 *
 * When CXDB_TEST_ADDR is set (e.g., in CI with Docker Compose), tests use the external
 * servers instead of spawning local instances.
 */
export default defineConfig({
  testDir: './tests',

  // Sequential execution for integration tests with server state
  fullyParallel: false,

  // Fail the build on CI if test.only is left in the source code
  forbidOnly: !!process.env.CI,

  // Retry on CI only
  retries: process.env.CI ? 2 : 0,

  // Single worker for integration tests (each test manages its own server)
  workers: 1,

  // Reporter configuration
  reporter: process.env.CI ? 'github' : 'html',

  // Timeout configuration
  timeout: 60000, // 60s per test (server startup can be slow)
  expect: {
    timeout: 10000, // 10s for assertions
  },

  use: {
    // Base URL for the Next.js dev server
    baseURL: frontendURL,

    // Collect trace on first retry
    trace: 'on-first-retry',

    // Screenshot on failure
    screenshot: 'only-on-failure',

    // Video on failure
    video: 'on-first-retry',
  },

  // Start the Next.js dev server before tests
  webServer: {
    command: `CXDB_NEXT_DIST_DIR=.next-playwright-${frontendPort} pnpm exec next dev --hostname 127.0.0.1 --port ${frontendPort}`,
    url: frontendURL,
    reuseExistingServer: !process.env.CI,
    timeout: 120000, // 2 minutes to start
  },

  // Global setup/teardown for building binaries (skipped when using external servers)
  globalSetup: useExternalServer ? undefined : './tests/global-setup.ts',
  globalTeardown: useExternalServer ? undefined : './tests/global-teardown.ts',

  // Test projects (browsers)
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    // Uncomment to test on more browsers
    // {
    //   name: 'firefox',
    //   use: { ...devices['Desktop Firefox'] },
    // },
    // {
    //   name: 'webkit',
    //   use: { ...devices['Desktop Safari'] },
    // },
  ],
});
