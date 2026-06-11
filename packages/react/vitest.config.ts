import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    // The first test in a file absorbs jsdom and React warm-up, which puts
    // it over the 5s default on a loaded machine.
    testTimeout: 15000,
  },
});
