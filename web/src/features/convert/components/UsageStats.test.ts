import { render, screen } from '@testing-library/vue';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createI18n } from 'vue-i18n';

import en from '../locales/en.json';
import viMessages from '../locales/vi.json';
import { loadUsageStats, type UsageStats as UsageStatsData } from '../stores/usageStats';

import UsageStats from './UsageStats.vue';

vi.mock('../stores/usageStats', () => ({ loadUsageStats: vi.fn() }));

const load = vi.mocked(loadUsageStats);

const TOTALS: UsageStatsData = { since: '2026-10-29', people: 1234, succeeded: 5678, failed: 90, contributed: 12 };

function show(locale: 'en' | 'vi') {
  const i18n = createI18n({ legacy: false, locale, fallbackLocale: 'en', messages: { en, vi: viMessages } });
  return render(UsageStats, { global: { plugins: [i18n] } });
}

afterEach(() => {
  load.mockReset();
});

describe('UsageStats', () => {
  it('shows the four totals and the start date in English', async () => {
    load.mockResolvedValueOnce(TOTALS);
    show('en');
    expect(await screen.findByText('1,234')).toBeInTheDocument();
    expect(screen.getByText('5,678')).toBeInTheDocument();
    expect(screen.getByText('90')).toBeInTheDocument();
    expect(screen.getByText('12')).toBeInTheDocument();
    expect(screen.getByText(en.convert.stats.succeeded)).toBeInTheDocument();
    expect(screen.getByText(/October 29, 2026/)).toBeInTheDocument();
  });

  it('formats the numbers for Vietnamese', async () => {
    load.mockResolvedValueOnce(TOTALS);
    show('vi');
    expect(await screen.findByText('1.234')).toBeInTheDocument();
    expect(screen.getByText('5.678')).toBeInTheDocument();
    expect(screen.getByText(viMessages.convert.stats.succeeded)).toBeInTheDocument();
  });

  it('renders nothing, not even an error, without totals', async () => {
    load.mockResolvedValueOnce(null);
    const { container } = show('en');
    await vi.waitFor(() => {
      expect(load).toHaveBeenCalledTimes(1);
    });
    await Promise.resolve();
    expect(container).toBeEmptyDOMElement();
  });

  it('asks the server once and never again while the page is open', async () => {
    vi.useFakeTimers();
    try {
      load.mockResolvedValue(TOTALS);
      show('en');
      await vi.advanceTimersByTimeAsync(10 * 60 * 1000);
      expect(load).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
