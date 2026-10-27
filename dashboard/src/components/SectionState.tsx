import type { ResourceHandle } from '../api/useResource';

/** A labelled figure under a list (e.g. "Fallback rate"). */
export type Stat = { label: string; value: string };

/** What a section shows: its data, or its loading, empty or error state. */
export type SectionStatus =
  | { kind: 'ready' }
  | { kind: 'loading' }
  | { kind: 'empty'; message: string }
  | { kind: 'error'; message: string; onRetry: () => void };

/** The status of a section drawn from one dataset; `isEmpty` decides when there is nothing to show. */
export function statusOf<T>(
  handle: ResourceHandle<T>,
  isEmpty: (data: T) => boolean,
  emptyMessage: string,
): SectionStatus {
  const { resource, retry } = handle;
  if (resource.status === 'loading') return { kind: 'loading' };
  if (resource.status === 'error') return { kind: 'error', message: resource.message, onRetry: retry };
  return isEmpty(resource.data) ? { kind: 'empty', message: emptyMessage } : { kind: 'ready' };
}

/** The body of a section that has no data to draw. Renders nothing when the section is ready. */
export function SectionState({ status }: { status: SectionStatus }) {
  switch (status.kind) {
    case 'ready':
      return null;
    case 'loading':
      return (
        <p class="ts-state" role="status">
          Loading…
        </p>
      );
    case 'empty':
      return <p class="ts-state">{status.message}</p>;
    case 'error':
      return (
        <div class="ts-state ts-state--error" role="alert">
          <span>Couldn't load this section. {status.message}</span>
          <button type="button" class="ts-retry" onClick={status.onRetry}>
            Retry
          </button>
        </div>
      );
  }
}
