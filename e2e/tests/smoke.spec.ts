import { test, expect } from '@playwright/test';

test('login page renders', async ({ page }) => {
  await page.goto('/login.html', { waitUntil: 'domcontentloaded' });
  await expect(page.locator('#login-form')).toBeVisible();
  await expect(page.locator('#username')).toBeVisible();
  await expect(page.locator('#password')).toBeVisible();
  await expect(page.locator('#btn-login')).toBeVisible();
});

test.describe('authenticated', () => {
  test.use({ storageState: '.auth/state.json' });

  test('desktop UI loads', async ({ page }) => {
    await page.goto('/', { waitUntil: 'domcontentloaded' });
    await expect(page.locator('#root')).toBeVisible();
    // `#root` can render mostly non-text UI (icons/canvas/video), so innerText can be empty.
    await expect
      .poll(async () => page.locator('#root').evaluate((el) => el.childElementCount), { timeout: 30_000 })
      .toBeGreaterThan(0);
  });

  test('health page loads', async ({ page }) => {
    await page.goto('/health.html', { waitUntil: 'domcontentloaded' });
    await expect(page).toHaveTitle(/health/i);
    await expect(page.locator('body')).toContainText(/SD|Health|Temperature|CPU/i);
  });

  test('leader key endpoint exists', async ({ playwright }) => {
    const api = await playwright.request.newContext({ storageState: '.auth/state.json' });
    const res = await api.get('/api/hid/leader-key');
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.code).toBe(0);
    expect(body.data).toHaveProperty('key');
    await api.dispose();
  });
});

test('websocket endpoint is reachable', async ({ request }) => {
  // Basic reachability check; full WS behavior is covered by UI usage.
  // Some servers may respond 404/405 to plain HTTP GETs; accept 200/101/400 as "present".
  const res = await request.get('/api/ws');
  expect([200, 101, 400, 401, 403, 405, 426]).toContain(res.status());
});
