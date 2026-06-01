import { request, type FullConfig, expect } from '@playwright/test';

function requiredEnv(name: string, fallback?: string): string {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`Missing required env var: ${name}`);
  return v;
}

async function sleep(ms: number) {
  await new Promise((r) => setTimeout(r, ms));
}

function encryptPassword(password: string, secretKey: string): string {
  const crypto = require('crypto') as typeof import('crypto');
  const salt = crypto.randomBytes(8);
  let derived = Buffer.alloc(0);
  let block = Buffer.alloc(0);
  while (derived.length < 48) {
    const h = crypto.createHash('md5');
    if (block.length) h.update(block);
    h.update(Buffer.from(secretKey, 'utf8'));
    h.update(salt);
    block = h.digest();
    derived = Buffer.concat([derived, block]);
  }
  const key = derived.subarray(0, 32);
  const iv = derived.subarray(32, 48);
  const cipher = crypto.createCipheriv('aes-256-cbc', key, iv);
  const enc = Buffer.concat([cipher.update(password, 'utf8'), cipher.final()]);
  return Buffer.concat([Buffer.from('Salted__'), salt, enc]).toString('base64');
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
      const keyRes = await api.get('/api/auth/encryption-key');
      expect(keyRes.status()).toBe(200);
      const keyBody = await keyRes.json();
      const secretKey = keyBody?.data?.key as string | undefined;
      expect(secretKey, 'Expected encryption key from /api/auth/encryption-key').toBeTruthy();
      const encrypted = encryptPassword(password, secretKey!);
      res = await api.post('/api/login', { data: { username, password: encrypted } });
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
