// GET a JSON resource of the dashboard API. Paths are relative (/api/...), so the page works
// on whatever loopback address and port the tokensave server binds.

export type Query = Record<string, string | number | undefined>;

/** A failed request, with the HTTP status (0 when the server could not be reached). */
export class ApiError extends Error {
  constructor(
    readonly path: string,
    readonly status: number,
    detail: string,
  ) {
    super(`GET ${path} failed: ${detail}`);
    this.name = 'ApiError';
  }
}

/** Builds `path?k=v`, leaving out empty values so "all" filters are not sent. */
export function apiUrl(path: string, query: Query = {}): string {
  const q = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== '') q.set(key, String(value));
  }
  const text = q.toString();
  return text ? `${path}?${text}` : path;
}

export async function getJson<T>(path: string, query: Query, signal: AbortSignal): Promise<T> {
  const url = apiUrl(path, query);
  let res: Response;
  try {
    res = await fetch(url, { signal, headers: { Accept: 'application/json' } });
  } catch (err) {
    if (signal.aborted) throw err;
    throw new ApiError(url, 0, 'the tokensave server could not be reached');
  }
  if (!res.ok) {
    // The server names the bad parameter in its body; keep it short enough for a card.
    const body = (await res.text().catch(() => '')).trim().slice(0, 200);
    throw new ApiError(url, res.status, `${res.status} ${res.statusText}${body ? ` (${body})` : ''}`);
  }
  try {
    return (await res.json()) as T;
  } catch {
    throw new ApiError(url, res.status, 'the response was not JSON');
  }
}

/** One endpoint of the API: its path, the query it takes from the page's state, and its type. */
export type Endpoint<T, S> = {
  url: (scope: S) => string;
  fetch: (scope: S, signal: AbortSignal) => Promise<T>;
};

export function endpoint<T, S = unknown>(path: string, query: (scope: S) => Query = () => ({})): Endpoint<T, S> {
  return {
    url: (scope) => apiUrl(path, query(scope)),
    fetch: (scope, signal) => getJson<T>(path, query(scope), signal),
  };
}
