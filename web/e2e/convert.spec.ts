import { writeFileSync } from 'node:fs';
import { join } from 'node:path';

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

const OPEN_REPORT = "File doesn't open in Revit?";
const REPORT_CONSENT = /^Send the original file, the converted file and the conversion log\./;
const REPORT_CODE = /Your report code is R-[0-9A-Z]{6}\./;

test('report_unopenable_with_the_file_this_tab_holds', async ({ page, browser, request, clientIp }) => {
  const served = async () => (await (await request.get('/api/stats')).json()) as Totals;
  const start = await served();
  await choose(page, revitFile('model_2026.rvt', 2026));
  await page.getByRole('button', { name: 'Downgrade' }).click();
  const download = page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ });
  await expect(download).toBeVisible();
  await expect.poll(async () => (await served()).succeeded).toBe(start.succeeded + 1);
  const before = await served();

  // Three actions: open, consent, send. The file is not chosen again.
  await page.getByRole('button', { name: OPEN_REPORT }).click();
  await page.getByRole('checkbox', { name: REPORT_CONSENT }).check();
  await expect(page.getByText('This page no longer holds your original file.', { exact: false })).toHaveCount(0);
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText(REPORT_CODE)).toBeVisible();
  await expect(download).toBeVisible();

  const after = await served();
  expect(after).toEqual({
    ...before,
    succeeded: before.succeeded - 1,
    failed: before.failed + 1,
    contributed: before.contributed + 1,
  });

  // A reloaded tab shows the code and offers no second report.
  await page.reload();
  await expect(page.getByText(REPORT_CODE)).toBeVisible();
  await expect(page.getByRole('button', { name: OPEN_REPORT })).toHaveCount(0);

  // The home page of a fresh browser shows the server's totals.
  const context = await browser.newContext({ extraHTTPHeaders: { 'X-Forwarded-For': clientIp } });
  try {
    const fresh = await context.newPage();
    await fresh.goto(new URL('/', page.url()).href);
    expect(await shownTotals(fresh)).toEqual(Object.fromEntries(COUNTS.map((key) => [key, after[key]])));
  } finally {
    await context.close();
  }
});

test('report_unopenable_after_reload_asks_for_the_original', async ({ page, request, filesDir }) => {
  const original = revitFile('model_2026.rvt', 2026);
  const wrong = join(filesDir, 'other_size.rvt');
  writeFileSync(wrong, Buffer.alloc(1234, 7));
  await choose(page, original);
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  const id = /\/job\/([0-9a-f]{32})$/.exec(page.url())?.[1] ?? '';

  await page.reload();
  await page.getByRole('button', { name: OPEN_REPORT }).click();
  await page.getByRole('checkbox', { name: REPORT_CONSENT }).check();
  const picker = page.getByLabel(/^This page no longer holds your original file\./);
  await picker.setInputFiles(wrong);
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText(
    'This is not the original file of this conversion. Choose the file you converted.',
  );
  const job = (await (await request.get(`/api/jobs/${id}`)).json()) as { report_code: string | null };
  expect(job.report_code).toBeNull();

  await picker.setInputFiles(original);
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText(REPORT_CODE)).toBeVisible();
});
