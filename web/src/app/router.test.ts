import { render, screen } from '@testing-library/vue';
import { createPinia } from 'pinia';
import { describe, expect, it, vi } from 'vitest';
import { createMemoryHistory } from 'vue-router';

import App from '@/App.vue';
import aboutEn from '@/features/about/locales/en.json';
import convertEn from '@/features/convert/locales/en.json';

import { createAppI18n } from './i18n';
import { createAppRouter } from './router';

// The convert view asks the server about a remembered or linked job; here the server knows nothing.
vi.mock('@/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api')>();
  return { ...actual, getJob: vi.fn().mockRejectedValue(new actual.ApiError(404, 'not_found', 'unknown')) };
});

async function open(path: string) {
  const router = createAppRouter(createMemoryHistory());
  await router.push(path);
  await router.isReady();
  const i18n = createAppI18n('en-US', { getItem: () => null, setItem: () => undefined });
  render(App, { global: { plugins: [createPinia(), router, i18n] } });
  return router;
}

describe('router', () => {
  it('renders the convert view at /', async () => {
    const router = await open('/');
    expect(router.currentRoute.value.name).toBe('convert');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(convertEn.convert.title);
  });

  it('renders the about view at /how-it-works', async () => {
    const router = await open('/how-it-works');
    expect(router.currentRoute.value.name).toBe('about');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(aboutEn.about.title);
  });

  it('renders the convert view for /job/:id', async () => {
    const router = await open('/job/0123456789abcdef0123456789abcdef');
    expect(router.currentRoute.value.name).toBe('job');
    expect(router.currentRoute.value.params.id).toBe('0123456789abcdef0123456789abcdef');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(convertEn.convert.title);
  });

  it('sends an unknown address back to the start', async () => {
    const router = await open('/no/such/page');
    expect(router.currentRoute.value.name).toBe('convert');
    expect(router.currentRoute.value.path).toBe('/');
  });
});
