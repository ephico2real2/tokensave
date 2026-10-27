import type { ComponentChildren } from 'preact';
import { SectionState, type SectionStatus, type Stat } from './SectionState';

/**
 * row-list: rows that read as a table on a wide screen and stack into cards on a phone
 * (each row wraps, key figure first). The rows' own markup comes from `renderRow`.
 */
export function RowList<T>({
  label,
  dataset,
  eyebrow,
  title,
  note,
  rows,
  rowKey,
  renderRow,
  layout = 'lines',
  stats,
  status,
  class: extraClass,
}: {
  label?: string;
  dataset: string;
  eyebrow?: string;
  title: string;
  note?: string;
  rows: T[];
  rowKey: (row: T) => string;
  renderRow: (row: T) => ComponentChildren;
  /** lines: rows divided by rules, in a list under the heading; boxes: one raised box per row; table: rules straight under the heading. */
  layout?: 'lines' | 'boxes' | 'table';
  stats?: Stat[];
  status: SectionStatus;
  class?: string;
}) {
  const ready = status.kind === 'ready';
  const items = rows.map((r) => <RowSlot key={rowKey(r)}>{renderRow(r)}</RowSlot>);
  return (
    <section
      aria-label={label}
      aria-busy={status.kind === 'loading'}
      data-component="row-list"
      data-dataset={dataset}
      class={`ts-card${layout === 'table' ? ' ts-card--table' : ''}${extraClass ? ` ${extraClass}` : ''}`}
    >
      {eyebrow && <span class="ts-eyebrow">{eyebrow}</span>}
      <h2 class="ts-h2">{title}</h2>
      {note && <span class="ts-note">{note}</span>}
      {!ready && <SectionState status={status} />}
      {ready && layout === 'lines' && <div class="ts-rows">{items}</div>}
      {ready && layout !== 'lines' && items}
      {ready && stats && (
        <div class="ts-stats">
          {stats.map((s) => (
            <div key={s.label} class="ts-stats__row">
              <span class="ts-muted">{s.label}</span>
              <span class="ts-mono">{s.value}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

/** Keyed wrapper so each row keeps its identity across renders; it adds no element. */
function RowSlot({ children }: { children: ComponentChildren }) {
  return <>{children}</>;
}
