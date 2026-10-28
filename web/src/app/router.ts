import { createRouter, createWebHistory, type RouterHistory } from 'vue-router';

import AboutView from '@/features/about/AboutView.vue';
import ConvertView from '@/features/convert/ConvertView.vue';
import { isPair } from '@/features/convert/pairs';
import { guideBySlug } from '@/features/guides/guides';
import GuidesView from '@/features/guides/GuidesView.vue';
import GuideView from '@/features/guides/GuideView.vue';

export function createAppRouter(history: RouterHistory = createWebHistory()) {
  return createRouter({
    history,
    routes: [
      { path: '/', name: 'convert', component: ConvertView },
      { path: '/how-it-works', name: 'about', component: AboutView },
      // One page per downgrade (/revit-2026-to-2024), so each can be found by searching for it.
      {
        path: '/revit-:from(\\d{4})-to-:to(\\d{4})',
        name: 'pair',
        component: ConvertView,
        beforeEnter: (to) => isPair(Number(to.params.from), Number(to.params.to)) || '/',
      },
      { path: '/guides', name: 'guides', component: GuidesView },
      {
        path: '/guides/:slug',
        name: 'guide',
        component: GuideView,
        beforeEnter: (to) => (guideBySlug(String(to.params.slug)) ? true : { name: 'guides' }),
      },
      { path: '/job/:id', name: 'job', component: ConvertView },
      { path: '/:unknown(.*)*', redirect: '/' },
    ],
  });
}
