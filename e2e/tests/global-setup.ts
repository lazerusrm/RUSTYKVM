import { request, type FullConfig, expect } from '@playwright/test';

function requiredEnv(name: string, fallback?: string): string {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`Missing required env var: ${name}`);
  return v;
}

async function sleep(ms: number) {
  await new Promise((r) => setTimeout(r, ms));
}

async function globalSetup(config: FullConfig) {
  const baseURL = requiredEnv('NANOKVM_BASE_URL', config.projects[0]?.use?.baseURL as string | undefined);
  const username = requiredEnv('NANOKVM_USER', 'admin');
  const password = requiredEnv('NANOKVM_PASS', 'admin');

  // Prefer API login for stability (UI login has client-side redirects/timeouts).
  const api = await request.newContext({ baseURL, ignoreHTTPSErrors: true });
  let lastErr: unknown = null;
  let res: Awaited<ReturnType<typeof api.post>> | null = null;
  for (let i = 0; i < 20; i++) {
    try {
      res = await api.post('/api/login', { data: { username, password } });
      break;
    } catch (e) {
      lastErr = e;
      await sleep(1000);
    }
  }
  if (!res) throw lastErr;

  expect(res.status(), 'Expected /api/login to return HTTP 200').toBe(200);

  const state = await api.storageState();
  const tokenCookie = state.cookies.find((c) => c.name === 'nano-kvm-token');
  expect(tokenCookie, 'Expected nano-kvm-token cookie to be set after login').toBeTruthy();

  await api.storageState({ path: '.auth/state.json' });
  await api.dispose();
}

export default globalSetup;
