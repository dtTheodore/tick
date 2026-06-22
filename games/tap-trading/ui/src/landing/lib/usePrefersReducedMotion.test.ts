import { describe, expect, it } from 'bun:test';

describe('readPrefersReducedMotion', () => {
  it('reads the (prefers-reduced-motion: reduce) media query', async () => {
    const calls: string[] = [];
    // @ts-expect-error test shim for a non-DOM environment
    globalThis.window = {
      matchMedia: (q: string) => {
        calls.push(q);
        return { matches: true, addEventListener() {}, removeEventListener() {} } as unknown as MediaQueryList;
      },
    };
    const { readPrefersReducedMotion } = await import('./usePrefersReducedMotion');
    expect(readPrefersReducedMotion()).toBe(true);
    expect(calls[0]).toContain('prefers-reduced-motion');
  });
});
