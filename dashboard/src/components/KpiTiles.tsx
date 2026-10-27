import { SectionState, type SectionStatus } from './SectionState';

export type Tile = { label: string; value: string; note: string; color?: string };

/** kpi-tiles: a grid of label, big number and note. */
export function KpiTiles({
  label,
  dataset,
  tiles,
  status,
  wide,
}: {
  label: string;
  dataset: string;
  tiles: Tile[];
  status: SectionStatus;
  /** Wider tiles (the Savings tab's three). */
  wide?: boolean;
}) {
  return (
    <section
      aria-label={label}
      aria-busy={status.kind === 'loading'}
      data-component="kpi-tiles"
      data-dataset={dataset}
      class={wide ? 'ts-tiles ts-tiles--wide' : 'ts-tiles'}
    >
      {status.kind === 'ready' ? (
        tiles.map((t) => (
          <div key={t.label} class="ts-tile-card">
            <span class="ts-tile-card__label">{t.label}</span>
            <span class="ts-tile-card__value" style={t.color ? { color: t.color } : undefined}>
              {t.value}
            </span>
            <span class="ts-note">{t.note}</span>
          </div>
        ))
      ) : (
        <div class="ts-tile-card ts-tile-card--state">
          <SectionState status={status} />
        </div>
      )}
    </section>
  );
}
