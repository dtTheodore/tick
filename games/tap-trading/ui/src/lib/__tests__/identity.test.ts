import { test, expect, beforeEach } from 'bun:test';

const storage = new Map<string, string>();
(globalThis as { localStorage?: Storage }).localStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => {
    storage.set(k, v);
  },
  removeItem: (k: string) => {
    storage.delete(k);
  },
  clear: () => storage.clear(),
  key: () => null,
  length: 0,
} as Storage;
(globalThis as { crypto?: Crypto }).crypto = {
  randomUUID: () => '00000000-0000-4000-8000-000000000abc' as `${string}-${string}-${string}-${string}-${string}`,
} as Crypto;

beforeEach(() => storage.clear());

test('getAccountId returns the same UUID on repeat calls', async () => {
  const { getAccountId } = await import('../identity');
  const a = getAccountId();
  const b = getAccountId();
  expect(a).toBe(b);
});

test('getAccountId persists across module imports', async () => {
  const { getAccountId } = await import('../identity');
  const a = getAccountId();
  expect(storage.get('tick.accountId')).toBe(a);
});
