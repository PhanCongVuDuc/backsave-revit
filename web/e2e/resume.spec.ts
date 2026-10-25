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
  // The reopened page shows the normal card; nothing is said about the unfinished upload until its file is chosen.
  await expect(page.getByRole('button', { name: 'Add a Revit file' })).toBeVisible();
  await expect(page.getByText(/did not finish/)).toBeHidden();

  await page.getByLabel('Revit file').setInputFiles(path);
  // Nothing is sent until the user asks to continue.
  await expect(page.getByText('Continuing your earlier upload')).toBeVisible();
  await page.getByRole('button', { name: 'Continue upload' }).click();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
  // The same job was continued, not a new one created.
  expect(page.url()).toBe(address);
});

test('second_tab_does_not_touch_first_tab_upload', async ({ page, context }) => {
  // Tab A's upload is held mid-request until the test lets it through.
  let releaseUpload: () => void = () => undefined;
  const uploadHeld = new Promise<void>((resolve) => {
    releaseUpload = resolve;
  });
  let patchSeen = false;
  await page.route('**/api/uploads/**', async (route) => {
    if (route.request().method() === 'PATCH') {
      patchSeen = true;
      await uploadHeld;
    }
    await route.continue();
  });

  const path = revitFile('model_2026.rvt', 2026);
  await page.goto('/');
  await page.getByLabel('Revit file').setInputFiles(path);
  await page.getByRole('button', { name: 'Downgrade' }).click();
  await expect.poll(() => patchSeen).toBe(true);

  // Tab B opens the start page: the normal card, no word about tab A's upload.
  const second = await context.newPage();
  await second.goto('/');
  await expect(second.getByRole('button', { name: 'Add a Revit file' })).toBeVisible();
  await expect(second.getByText(/did not finish|another tab/)).toBeHidden();

  // Choosing the same file in tab B neither takes over nor deletes tab A's job.
  await second.getByLabel('Revit file').setInputFiles(path);
  await expect(second.getByRole('alert')).toHaveText('This file is being uploaded in another tab.');

  releaseUpload();
  await expect(page.getByRole('link', { name: /^Download model_2026_R2025\.rvt/ })).toBeVisible();
});
