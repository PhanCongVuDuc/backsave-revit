import { describe, expect, it } from 'vitest';

import { formatBytes, formatClock } from './format';

describe('formatBytes', () => {
  it('uses binary units with one decimal under 10', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(133 * 1024 * 1024)).toBe('133 MB');
    expect(formatBytes(1_073_741_824)).toBe('1 GB');
    expect(formatBytes(5.25 * 1024 * 1024)).toBe('5.3 MB');
  });
});

describe('formatClock', () => {
  it('writes minutes and two-digit seconds', () => {
    expect(formatClock(0)).toBe('0:00');
    expect(formatClock(65)).toBe('1:05');
    expect(formatClock(240)).toBe('4:00');
  });
});
