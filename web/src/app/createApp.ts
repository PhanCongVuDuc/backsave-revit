import { createPinia } from 'pinia';
import { createApp } from 'vue';

import App from '@/App.vue';

import { createAppI18n, safeStorage } from './i18n';
import { createAppRouter } from './router';

export function createBacksaveApp() {
  const app = createApp(App);
  app.use(createPinia());
  app.use(createAppRouter());
  app.use(createAppI18n(navigator.language, safeStorage()));
  return app;
}
