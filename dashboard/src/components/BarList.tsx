import { SectionState, type SectionStatus, type Stat } from './SectionState';

export type BarRow = {
  name: string;
  value: string;
  /** Bar length, a CSS percentage. */
  pct: string;
  /** Category colour of the dot and bar; rows without one have no dot. */
  color?: string;
  /** Bar colour when it differs from the dot (or there is no dot). */
  barColor?: string;
  mono?: boolean;
};

/** bar-list: one row per item with its name, value, category dot and bar. */
export function BarList({
  as = 'section',
  label,
  dataset,
  eyebrow,
  title,
  rows,
  note,
  stats,
  status,
}: {
  as?: 'section' | 'aside';
  label?: string;
  dataset: string;
  eyebrow: string;
  title: string;
  rows: BarRow[];
  note?: string;
  stats?: Stat[];
  status: SectionStatus;
}) {
  const Tag = as;
  return (
    <Tag
      aria-label={label}
      aria-busy={status.kind === 'loading'}
      data-component="bar-list"
      data-dataset={dataset}
      class="ts-card"
    >
      <span class="ts-eyebrow">{eyebrow}</span>
      <h2 class="ts-h2">{title}</h2>
      {status.kind === 'ready' ? <Rows rows={rows} /> : <SectionState status={status} />}
      {note && <span class="ts-note">{note}</span>}
      {status.kind === 'ready' && stats && (
        <div class="ts-stats">
          {stats.map((s) => (
            <div key={s.label} class="ts-stats__row">
              <span class="ts-muted">{s.label}</span>
              <span class="ts-mono">{s.value}</span>
            </div>
          ))}
        </div>
      )}
    </Tag>
  );
}

function Rows({ rows }: { rows: BarRow[] }) {
  return (
    <>
      {rows.map((r) => (
    <div key={r.name} class="ts-bar">
      <div class="ts-bar__head">
        <span class={r.mono ? 'ts-bar__name ts-mono' : 'ts-bar__name'}>
          {r.color && <span class="ts-dot" style={{ background: r.color }} />}
          {r.name}
        </span>
        <span class="ts-bar__value">{r.value}</span>
      </div>
      <div class="ts-bar__track">
        <div class="ts-bar__fill" style={{ background: r.barColor ?? r.color, width: r.pct }} />
      </div>
    </div>
      ))}
    </>
  );
}
