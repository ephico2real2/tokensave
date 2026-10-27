import { SectionState, type SectionStatus } from './SectionState';

export type StripStat = { label: string; value: string; color?: string };

/** stat-strip: one card with a heading figure and its note, and a few labelled numbers beside it. */
export function StatStrip({
  eyebrow,
  value,
  note,
  stats,
  status,
}: {
  eyebrow: string;
  value: string;
  note: string;
  stats: StripStat[];
  status: SectionStatus;
}) {
  return (
    <section
      aria-label={eyebrow}
      aria-busy={status.kind === 'loading'}
      data-component="stat-strip"
      data-dataset="repos"
      class="ts-card ts-strip"
    >
      <div class="ts-strip__main">
        <span class="ts-eyebrow">{eyebrow}</span>
        {status.kind === 'ready' ? (
          <>
            <span class="ts-strip__value ts-mono">{value}</span>
            <span class="ts-note">{note}</span>
          </>
        ) : (
          <SectionState status={status} />
        )}
      </div>
      {status.kind === 'ready' && (
        <div class="ts-strip__stats">
          {stats.map((s) => (
            <div key={s.label} class="ts-figure">
              <span class="ts-figure__label">{s.label}</span>
              <span class="ts-strip__stat ts-mono" style={s.color ? { color: s.color } : undefined}>
                {s.value}
              </span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
