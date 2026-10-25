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
