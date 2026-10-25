import { render, screen } from '@testing-library/vue';
import { describe, expect, it } from 'vitest';
import { createMemoryHistory } from 'vue-router';

import App from '@/App.vue';
import en from '@/shared/locales/en.json';

import { createAppI18n } from './i18n';
import { createAppRouter } from './router';

async function open(path: string) {
  const router = createAppRouter(createMemoryHistory());
  await router.push(path);
  await router.isReady();
  const i18n = createAppI18n('en-US', { getItem: () => null, setItem: () => undefined });
  render(App, { global: { plugins: [router, i18n] } });
  return router;
}

describe('router', () => {
  it('renders the convert view at /', async () => {
    const router = await open('/');
    expect(router.currentRoute.value.name).toBe('convert');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(en.convert.title);
  });

  it('renders the about view at /how-it-works', async () => {
    const router = await open('/how-it-works');
    expect(router.currentRoute.value.name).toBe('about');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(en.about.title);
  });

  it('renders the convert view for /job/:id', async () => {
    const router = await open('/job/0123456789abcdef0123456789abcdef');
    expect(router.currentRoute.value.name).toBe('job');
    expect(router.currentRoute.value.params.id).toBe('0123456789abcdef0123456789abcdef');
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(en.convert.title);
  });
});
