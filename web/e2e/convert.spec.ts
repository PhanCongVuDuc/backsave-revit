import { type Page } from '@playwright/test';

import { expect, revitFile, test } from './fixtures';

async function choose(page: Page, path: string) {
  await page.goto('/');
  await page.getByLabel('Revit file', { exact: true }).setInputFiles(path);
}

test('downgrade_rvt_and_download', async ({ page, clientIp }) => {
  const tusHeaders: (string | undefined)[] = [];
  page.on('request', (request) => {
    if (request.url().includes('/api/uploads/')) {
      void request.allHeaders().then((headers) => tusHeaders.push(headers['x-forwarded-for']));
    }
  });

  await choose(page, revitFile('model_2026.rvt', 2026));
  await expect(page.getByText('Revit 2026 · Project')).toBeVisible();
  await page.getByRole('group', { name: 'Target Revit version' }).getByText('Revit 2024').click();
  await page.getByRole('button', { name: 'Downgrade' }).click();

  const download = page.getByRole('link', { name: /^Download model_2026_R2024\.rvt/ });
  await expect(download).toBeVisible();
  await expect(page.getByRole('heading', { name: '72 elements were removed' })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'CustomElement' })).toBeVisible();
  await expect(page.getByRole('cell', { name: '61', exact: true })).toBeVisible();

  const [file] = await Promise.all([page.waitForEvent('download'), download.click()]);
  expect(file.suggestedFilename()).toBe('model_2026_R2024.rvt');

  // The per-client header reaches the upload requests as well as the API calls.
  expect(tusHeaders.length).toBeGreaterThan(0);
  expect(tusHeaders.every((value) => value === clientIp)).toBe(true);
});

test('rfa_family_flow', async ({ page }) => {
  await choose(page, revitFile('family_2025.rfa', 2025));
  await expect(page.getByText('Revit 2025 · Family')).toBeVisible();
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect(page.getByRole('link', { name: /^Download family_2025_R2024\.rfa/ })).toBeVisible();
  await expect(page.getByText('Converted to Revit 2024')).toBeVisible();
});

test('old_file_rejected_without_request', async ({ page }) => {
  const jobRequests: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/api/jobs')) {
      jobRequests.push(request.url());
    }
  });
  await choose(page, revitFile('old_2022.rvt', 2022));
  await expect(page.getByText('Revit 2022 files cannot be downgraded yet. Nothing was sent.')).toBeVisible();
  expect(jobRequests).toEqual([]);
});

test('delete_now_removes_the_job', async ({ page, request }) => {
  await choose(page, revitFile('model_2026.rvt', 2026));
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  const id = /\/job\/([0-9a-f]{32})$/.exec(page.url())?.[1];
  expect(id).toBeDefined();
  expect((await request.get(`/api/jobs/${id ?? ''}`)).status()).toBe(200);

  await page.getByRole('button', { name: 'Delete now' }).click();
  await expect(page.getByRole('button', { name: 'Add a Revit file' })).toBeVisible();

  // Deleting removes the job outright, so the server answers 404; 410 would mean it was kept as expired.
  const after = await request.get(`/api/jobs/${id ?? ''}`);
  expect(after.status()).toBe(404);
});

const COUNTS = ['people', 'succeeded', 'failed', 'contributed'] as const;

type Totals = Record<(typeof COUNTS)[number], number>;

/** The four totals as the home page shows them (in the order people, succeeded, failed, contributed). */
async function shownTotals(page: Page): Promise<Totals> {
  const totals = page.getByRole('region', { name: 'Usage so far' });
  await expect(totals).toBeVisible();
  await expect(totals.getByText('files downgraded')).toBeVisible();
  const texts = await totals.getByRole('definition').allTextContents();
  expect(texts).toHaveLength(COUNTS.length);
  return Object.fromEntries(COUNTS.map((key, i) => [key, Number((texts[i] ?? '').replaceAll(',', ''))])) as Totals;
}

test('usage_stats_shown_without_interaction', async ({ page }) => {
  await page.goto('/');
  const totals = page.getByRole('region', { name: 'Usage so far' });
  await expect(totals).toBeVisible();
  await expect(totals.getByRole('definition')).toHaveCount(4);
  // The succeeded total is visible with its label, with no interaction.
  await expect(totals.getByText('files downgraded')).toBeVisible();
  await expect(totals.getByRole('definition').nth(1)).toBeVisible();
  await expect(totals.getByRole('definition').nth(1)).toHaveText(/^\d[\d,]*$/);
  await expect(totals.getByText(/^Counted since /)).toBeVisible();
});

test('usage_stats_count_a_downgrade', async ({ page, browser, request, clientIp }) => {
  await page.goto('/');
  const before = await shownTotals(page);

  await choose(page, revitFile('model_2026.rvt', 2026));
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();

  // The count is written just after the job is done; wait for the server to show it.
  const served = async () => (await (await request.get('/api/stats')).json()) as Totals;
  await expect.poll(async () => (await served()).succeeded).toBe(before.succeeded + 1);
  const totals = await served();

  // A fresh browser context: the totals may be cached for 60 s, so a reload could show the old ones.
  const context = await browser.newContext({ extraHTTPHeaders: { 'X-Forwarded-For': clientIp } });
  try {
    const fresh = await context.newPage();
    await fresh.goto(new URL('/', page.url()).href);
    // Every number on the page is the server's, in its own place.
    expect(await shownTotals(fresh)).toEqual(Object.fromEntries(COUNTS.map((key) => [key, totals[key]])));
  } finally {
    await context.close();
  }
});

test('usage_stats_hidden_when_unavailable', async ({ page }) => {
  await page.route('**/api/stats', (route) =>
    route.fulfill({
      status: 503,
      json: {
        error: { code: 'stats_unavailable', message: 'Usage statistics are not available.', retry_after_s: null },
      },
    }),
  );
  const answered = page.waitForResponse('**/api/stats');
  await page.goto('/');
  await answered;
  await expect(page.getByRole('button', { name: 'Add a Revit file' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Usage so far' })).toHaveCount(0);
  await expect(page.getByText(/statistics/i)).toHaveCount(0);
});
