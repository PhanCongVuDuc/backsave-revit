import { join } from 'node:path';

import vitest from '@vitest/eslint-plugin';
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript';
import skipFormatting from 'eslint-config-prettier/flat';
import boundaries from 'eslint-plugin-boundaries';
import playwright from 'eslint-plugin-playwright';
import testingLibrary from 'eslint-plugin-testing-library';
import unicorn from 'eslint-plugin-unicorn';
import pluginVue from 'eslint-plugin-vue';

const SOURCE_FILES = ['src/**/*.{ts,vue}'];
const TEST_FILES = ['**/*.test.ts', 'e2e/**/*.spec.ts'];
const VITEST_FILES = ['**/*.test.ts'];
const COMPONENT_TEST_FILES = ['src/**/*.test.ts'];
const E2E_FILES = ['e2e/**/*.spec.ts'];

export default defineConfigWithVueTs(
  {
    name: 'backsave/ignores',
    ignores: ['dist/**', 'node_modules/**', 'test-results/**', 'playwright-report/**'],
  },
  pluginVue.configs['flat/recommended'],
  vueTsConfigs.strictTypeChecked,
  {
    name: 'backsave/code',
    rules: {
      '@typescript-eslint/no-floating-promises': 'error',
      '@typescript-eslint/switch-exhaustiveness-check': 'error',
      '@typescript-eslint/no-non-null-assertion': 'error',
      'no-nested-ternary': 'error',
      'vue/block-order': ['error', { order: ['script', 'template', 'style'] }],
      'vue/define-macros-order': [
        'error',
        { order: ['defineOptions', 'defineProps', 'defineEmits', 'defineModel', 'defineSlots'] },
      ],
      'vue/component-name-in-template-casing': ['error', 'PascalCase'],
      'vue/custom-event-name-casing': ['error', 'camelCase'],
      'vue/require-explicit-emits': 'error',
      'vue/no-unused-refs': 'error',
      'vue/no-v-html': 'error',
    },
  },
  // checkDirectories is off because folders are kebab-case while files are camelCase or PascalCase.
  {
    name: 'backsave/file-names',
    files: ['src/**/*.{ts,vue}', 'e2e/**/*.ts'],
    plugins: { unicorn },
    rules: {
      'unicorn/filename-case': ['error', { cases: { camelCase: true }, checkDirectories: false }],
    },
  },
  {
    name: 'backsave/file-names-vue',
    files: ['src/**/*.vue'],
    rules: {
      'unicorn/filename-case': ['error', { cases: { pascalCase: true }, checkDirectories: false }],
    },
  },
  {
    name: 'backsave/file-names-tests',
    files: TEST_FILES,
    rules: {
      // A test file is named after the file it tests, which is PascalCase for a component.
      'unicorn/filename-case': ['error', { cases: { camelCase: true, pascalCase: true }, checkDirectories: false }],
    },
  },
  // Import boundaries:
  //  - src/features/<name>/ imports only itself and @/shared;
  //  - only src/features/<name>/stores/ and src/app/ import @/api;
  //  - @/shared imports only @/shared.
  {
    name: 'backsave/boundaries',
    files: SOURCE_FILES,
    plugins: { boundaries },
    settings: {
      'boundaries/root-path': import.meta.dirname,
      'import/resolver': { typescript: { project: join(import.meta.dirname, 'tsconfig.app.json') } },
      'boundaries/elements': [
        { type: 'app', pattern: 'src/app/**', partialMatch: false },
        { type: 'api', pattern: 'src/api/**', partialMatch: false },
        { type: 'shared', pattern: 'src/shared/**', partialMatch: false },
        { type: 'store', pattern: 'src/features/*/stores/**', partialMatch: false, capture: ['feature'] },
        { type: 'feature', pattern: 'src/features/*/**', partialMatch: false, capture: ['feature'] },
      ],
      'boundaries/files': [{ category: 'testing', pattern: 'src/shared/testing/**' }],
    },
    rules: {
      'boundaries/dependencies': [
        'error',
        {
          default: 'allow',
          policies: [
            {
              from: { element: { types: { anyOf: ['feature', 'store'] } } },
              disallow: {
                to: {
                  element: {
                    types: { anyOf: ['feature', 'store'] },
                    captured: { feature: '!{{ from.element.captured.feature }}' },
                  },
                },
              },
              message: 'A feature imports only its own folder and @/shared, never another feature.',
            },
            {
              from: { element: { types: { anyOf: ['feature', 'store'] } } },
              disallow: { to: { element: { type: 'app' } } },
              message: 'A feature does not import src/app/.',
            },
            {
              from: { element: { type: 'shared' } },
              disallow: { to: { element: { types: { anyOf: ['app', 'api', 'feature', 'store'] } } } },
              message: '@/shared imports only @/shared.',
            },
            {
              from: { element: { type: 'feature' } },
              disallow: { to: { element: { type: 'api' } } },
              message: 'Only a store (features/<name>/stores/) and src/app/ import @/api.',
            },
            {
              from: { element: { type: 'api' } },
              disallow: { to: { element: { types: { anyOf: ['app', 'feature', 'store'] } } } },
              message: '@/api does not import the app or a feature.',
            },
            // The test helpers build the whole app the way main.ts does.
            {
              from: { file: { categories: 'testing' } },
              allow: { to: { element: { types: { anyOf: ['app', 'api'] } } } },
            },
          ],
        },
      ],
    },
  },
  {
    name: 'backsave/restricted-features',
    files: ['src/features/**'],
    ignores: TEST_FILES,
    rules: {
      'no-restricted-imports': [
        'error',
        { paths: [{ name: 'reka-ui', message: 'Features use the wrappers in @/shared/ui, not reka-ui.' }] },
      ],
    },
  },
  {
    ...vitest.configs.recommended,
    name: 'backsave/tests-vitest',
    files: VITEST_FILES,
    rules: {
      ...vitest.configs.recommended.rules,
      'vitest/no-focused-tests': 'error',
      'vitest/no-disabled-tests': 'error',
      'vitest/warn-todo': 'error',
      'vitest/no-conditional-in-test': 'error',
      'vitest/no-conditional-expect': 'error',
      'vitest/prefer-to-have-length': 'error',
      'vitest/no-large-snapshots': 'error',
    },
  },
  {
    ...testingLibrary.configs['flat/vue'],
    name: 'backsave/tests-testing-library',
    files: COMPONENT_TEST_FILES,
  },
  {
    ...playwright.configs['flat/recommended'],
    name: 'backsave/tests-e2e',
    files: E2E_FILES,
    rules: {
      ...playwright.configs['flat/recommended'].rules,
      // Every e2e locator goes by role, label or text, never by CSS.
      'playwright/no-raw-locators': 'error',
    },
  },
  skipFormatting,
);
