import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/vue';
import { afterEach, beforeEach, expect } from 'vitest';

import { consoleWarnings } from './consoleWarnings';

// A warning nobody declared is usually a real bug, so it fails the test - as vuejs/core scripts/setup-vitest.ts does.
// A plain function rather than vi.spyOn: a test's own spy on console then takes the calls and declares them itself.
const realConsole = { warn: console.warn, error: console.error };

// A warning may carry an object with no toString (null prototype): String() would throw inside the code that warned.
function textOf(value: unknown): string {
  try {
    return String(value);
  } catch {
    return Object.prototype.toString.call(value);
  }
}

function recordConsoleCall(...args: unknown[]) {
  consoleWarnings.calls.push(args.map(textOf).join(' '));
}

function isDeclared(call: string) {
  return consoleWarnings.declared.some((declared) => call.includes(declared));
}

expect.extend({
  toHaveBeenWarned(received: string) {
    consoleWarnings.declared.push(received);
    const pass = consoleWarnings.calls.some((call) => call.includes(received));
    const callsSeen = consoleWarnings.calls.length === 0 ? '(none)' : consoleWarnings.calls.join('\n - ');
    return {
      pass,
      message: () => `expected a console warning containing "${received}"${pass ? ' not' : ''}; seen:\n - ${callsSeen}`,
    };
  },
});

beforeEach(() => {
  consoleWarnings.calls = [];
  consoleWarnings.declared = [];
  console.warn = recordConsoleCall;
  console.error = recordConsoleCall;
});

afterEach(() => {
  console.warn = realConsole.warn;
  console.error = realConsole.error;
  const unexpected = consoleWarnings.calls.filter((call) => !isDeclared(call));
  if (unexpected.length > 0) {
    throw new Error(`test case threw unexpected warnings:\n - ${unexpected.join('\n - ')}`);
  }
});

// Testing Library cleans up on its own only when Vitest globals are on; this app imports describe/it/expect instead.
afterEach(cleanup);
