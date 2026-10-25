import { type Page } from '@playwright/test';

import { expect, revitFile, test } from './fixtures';

async function choose(page: Page, path: string) {
  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(path);
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
  await page.getByLabel('Target Revit version').selectOption({ label: 'Revit 2024' });
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
  expect([404, 410]).toContain(after.status());
  expect(after.status()).toBe(404);
});
