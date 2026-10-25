import { defineConfig, mergeConfig } from 'vitest/config';

import viteConfig from './vite.config.ts';

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      clearMocks: true,
      restoreMocks: true,
      projects: [
        {
          extends: true,
          test: {
            name: 'unit',
            setupFiles: ['src/shared/testing/setup.ts'],
            environment: 'happy-dom',
            include: ['src/**/*.test.ts'],
            exclude: ['node_modules/**'],
          },
        },
      ],
    },
  }),
);
