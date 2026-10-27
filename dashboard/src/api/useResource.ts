import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { Endpoint } from './http';

export type Resource<T> =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; data: T };

export type ResourceHandle<T> = { resource: Resource<T>; retry: () => void };

/**
 * Loads one dataset and reloads it whenever `key` changes (the key is the request's identity:
 * the endpoint and its query). A newer request aborts the one in flight, so a slow answer for
 * an old range never overwrites the current one.
 */
export function useResource<T>(key: string, load: (signal: AbortSignal) => Promise<T>): ResourceHandle<T> {
  const [resource, setResource] = useState<Resource<T>>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    const controller = new AbortController();
    setResource({ status: 'loading' });
    loadRef.current(controller.signal).then(
      (data) => {
        if (!controller.signal.aborted) setResource({ status: 'ready', data });
      },
      (err: unknown) => {
        if (controller.signal.aborted) return;
        setResource({ status: 'error', message: err instanceof Error ? err.message : String(err) });
      },
    );
    return () => controller.abort();
  }, [key, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return { resource, retry };
}

/** Loads an endpoint for the page's current scope; it reloads when the endpoint's own query changes. */
export function useDataset<T, S>(api: Endpoint<T, S>, scope: S): ResourceHandle<T> {
  return useResource(api.url(scope), (signal) => api.fetch(scope, signal));
}

/** The loaded data, or undefined while loading or after an error. */
export const dataOf = <T,>(handle: ResourceHandle<T>): T | undefined =>
  handle.resource.status === 'ready' ? handle.resource.data : undefined;
