/** Articles under /guides/<slug>; `key` is where their text sits in the locale files. */
export const GUIDES = [
  { slug: 'revit-save-as-older-version', key: 'saveAsOlder' },
  { slug: 'open-newer-revit-file-in-older-revit', key: 'openInOlder' },
  { slug: 'revit-version-compatibility', key: 'compatibility' },
  { slug: 'what-changes-when-downgrading-revit', key: 'whatChanges' },
] as const;

export type Guide = (typeof GUIDES)[number];

export function guideBySlug(slug: string): Guide | undefined {
  return GUIDES.find((g) => g.slug === slug);
}

export function guidePath(guide: Guide): string {
  return `/guides/${guide.slug}`;
}
