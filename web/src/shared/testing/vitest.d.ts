import 'vitest';

declare module 'vitest' {
  interface Assertion {
    toHaveBeenWarned: () => void;
  }
}
