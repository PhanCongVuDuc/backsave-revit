import { type Page } from '@playwright/test';

import { expect, revitFile, test } from './fixtures';

async function convert(page: Page, path: string) {
  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(path);
  await page.getByRole('button', { name: 'Downgrade' }).click();
}

test('refused_shows_engine_message', async ({ page }) => {
  await convert(page, revitFile('x_refuse_2026.rvt', 2026, 'refuse'));
  await expect(
    page.getByRole('alert').filter({ hasText: 'Not supported yet: Worksharing central files are not supported yet' }),
  ).toBeVisible();
});

test('corrupt_file_is_reported', async ({ page }) => {
  await convert(page, revitFile('x_corrupt_2026.rvt', 2026, 'corrupt'));
  await expect(
    page.getByRole('alert').filter({ hasText: 'The file could not be read. It may be damaged.' }),
  ).toBeVisible();
});

test('failed_offers_report_default_unticked', async ({ page }) => {
  await convert(page, revitFile('x_fail_2026.rvt', 2026, 'fail'));
  await expect(
    page.getByRole('alert').filter({ hasText: 'The conversion failed. The problem has been logged.' }),
  ).toBeVisible();

  const consent = page.getByRole('checkbox');
  await expect(consent).not.toBeChecked();
  const send = page.getByRole('button', { name: 'Send' });
  await expect(send).toBeDisabled();

  await consent.check();
  await expect(send).toBeEnabled();
  await send.click();
  await expect(page.getByText(/Your report code is R-[0-9A-Z]{6}\./)).toBeVisible();
});
