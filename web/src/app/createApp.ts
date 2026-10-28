import { createPinia } from 'pinia';
import { createApp } from 'vue';

import App from '@/App.vue';
import { safeStorage } from '@/shared/storage/safeStorage';

import { createAppI18n } from './i18n';
import { createAppRouter } from './router';

export function createRevitDowngradeApp() {
  const app = createApp(App);
  app.use(createPinia());
  app.use(createAppRouter());
  app.use(createAppI18n(navigator.language, safeStorage()));
  return app;
}
