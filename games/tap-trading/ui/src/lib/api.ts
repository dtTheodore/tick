import { env } from './env';
import { getAccountId } from './identity';

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    public body?: unknown,
    public retryAfterSecs?: number,
  ) {
    super(`api ${status} ${code}`);
  }

  /** Server's recomputed μ from the drift_exceeded error body, if present. */
  get serverMultiplier(): number | undefined {
    if (this.body && typeof this.body === 'object' && 'server_multiplier' in this.body) {
      const v = (this.body as { server_multiplier: unknown }).server_multiplier;
      return typeof v === 'number' ? v : undefined;
    }
    return undefined;
  }
}

interface RequestOptions {
  method?: 'GET' | 'POST';
  body?: unknown;
  signal?: AbortSignal;
}

export async function api<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const res = await fetch(`${env.apiUrl}${path}`, {
    method: opts.method ?? 'GET',
    headers: {
      'X-Account-Id': getAccountId(),
      ...(opts.body ? { 'Content-Type': 'application/json' } : {}),
    },
    body: opts.body ? JSON.stringify(opts.body) : undefined,
    signal: opts.signal,
  });
  if (!res.ok) {
    let body: unknown = undefined;
    let code = String(res.status);
    try {
      body = await res.json();
      if (body && typeof body === 'object' && 'error' in body && typeof body.error === 'string') {
        code = body.error;
      }
    } catch {
      /* ignore parse failure */
    }
    const retryHeader = res.headers.get('retry-after');
    const retryAfterSecs = retryHeader ? Number(retryHeader) : undefined;
    throw new ApiError(
      res.status,
      code,
      body,
      Number.isFinite(retryAfterSecs) ? retryAfterSecs : undefined,
    );
  }
  return (await res.json()) as T;
}
