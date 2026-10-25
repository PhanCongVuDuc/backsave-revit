import { expect, revitFile, test } from './fixtures';

test('upload_resumes_after_network_drop', async ({ page }) => {
  let patches = 0;
  await page.route('**/api/uploads/**', async (route) => {
    if (route.request().method() === 'PATCH') {
      patches += 1;
      if (patches === 1) {
        await route.abort('connectionreset');
        return;
      }
    }
    await route.continue();
  });

  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(revitFile('model_2026.rvt', 2026));
  await page.getByRole('button', { name: 'Downgrade' }).click();

  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  // The first PATCH was dropped and tus sent the data again.
  expect(patches).toBeGreaterThanOrEqual(2);
});

test('reopen_tab_shows_result', async ({ page, context }) => {
  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(revitFile('model_2026.rvt', 2026));
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  const address = page.url();
  expect(address).toMatch(/\/job\/[0-9a-f]{32}$/);

  const reopened = await context.newPage();
  await reopened.goto(address);
  await expect(reopened.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  await expect(reopened.getByRole('heading', { name: '72 elements were removed' })).toBeVisible();
});

test('reload_mid_upload_then_continue', async ({ page }) => {
  // Every PATCH fails, so the upload cannot finish before the page is reloaded.
  let attempts = 0;
  await page.route('**/api/uploads/**', async (route) => {
    if (route.request().method() === 'PATCH') {
      attempts += 1;
      await route.abort('connectionreset');
      return;
    }
    await route.continue();
  });
  // Leaving during an upload asks for confirmation; accept it.
  page.on('dialog', (dialog) => void dialog.accept());

  const path = revitFile('model_2026.rvt', 2026);
  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(path);
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect.poll(() => attempts).toBeGreaterThan(0);
  await expect(page).toHaveURL(/\/job\/[0-9a-f]{32}$/);
  const address = page.url();

  await page.unroute('**/api/uploads/**');
  await page.reload();
  await expect(
    page.getByText('An upload of model_2026.rvt did not finish. Choose the same file to continue.'),
  ).toBeVisible();
  expect(page.url()).toBe(address);

  await page.getByLabel('Revit file').setInputFiles(path);
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  expect(page.url()).toBe(address);
});
