/** The console calls of the running test and the warnings it declared; reset by `setup.ts` before each test. */
export const consoleWarnings = {
  calls: [] as string[],
  declared: [] as string[],
};

/**
 * Declares a warning the test environment itself causes, so `setup.ts` does not fail the test on it. Call it in the test or in a
 * `beforeEach` of the file, with a comment saying who warns and why; a warning the test checks for is `expect(text).toHaveBeenWarned()`.
 */
export function allowWarning(text: string): void {
  consoleWarnings.declared.push(text);
}
