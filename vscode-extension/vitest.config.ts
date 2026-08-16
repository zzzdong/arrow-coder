import { defineConfig } from 'vitest/config';

// UI logic (stores, utils, protocol) is framework-agnostic TypeScript and is
// exercised under Node — no jsdom required for the pure state-machine tests.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['webview/src/**/*.test.ts'],
    globals: true,
  },
});
