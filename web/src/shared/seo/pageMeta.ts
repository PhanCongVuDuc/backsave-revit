/** Address the site is published at; canonical and share links point here. */
export const SITE_URL = 'https://revitdowngrade.com';

function setContent(selector: string, value: string) {
  document.head.querySelector(selector)?.setAttribute('content', value);
}

/** Title, description and canonical URL of the page on screen, also in the tags link previews read. */
export function setPageMeta({ title, description, path }: { title: string; description?: string; path: string }) {
  document.title = title;
  setContent('meta[property="og:title"]', title);
  const url = `${SITE_URL}${path}`;
  document.head.querySelector('link[rel="canonical"]')?.setAttribute('href', url);
  setContent('meta[property="og:url"]', url);
  if (description) {
    setContent('meta[name="description"]', description);
    setContent('meta[property="og:description"]', description);
  }
}
