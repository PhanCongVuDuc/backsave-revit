import { createRouter, createWebHistory, type RouterHistory } from 'vue-router';

import AboutView from '@/features/about/AboutView.vue';
import ConvertView from '@/features/convert/ConvertView.vue';

export function createAppRouter(history: RouterHistory = createWebHistory()) {
  return createRouter({
    history,
    routes: [
      { path: '/', name: 'convert', component: ConvertView },
      { path: '/how-it-works', name: 'about', component: AboutView },
      { path: '/job/:id', name: 'job', component: ConvertView },
    ],
  });
}
