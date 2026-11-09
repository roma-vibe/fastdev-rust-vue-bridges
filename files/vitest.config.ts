import { defineConfig } from 'vitest/config';

// Web tests and the tests of the project scripts; Rust tests run with `cargo test --workspace`
// (all of them: `npm test`).
export default defineConfig({
  test: {
    projects: [
      'web',
      { test: { name: 'scripts', include: ['scripts/**/*.test.ts'], environment: 'node' } },
    ],
  },
});
