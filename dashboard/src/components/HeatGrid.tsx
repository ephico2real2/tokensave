import { SectionState, type SectionStatus } from './SectionState';

export type HeatCell = { text: string; level: 0 | 1 | 2 | 3 | 'none'; label: string };
export type HeatRow = { name: string; color: string; cells: HeatCell[] };

/**
 * heat-grid: a 2-D grid with a sticky first column; it scrolls sideways inside its card.
 * The rows are ARIA table rows (display: contents), so a screen reader reads it as a table.
 */
export function HeatGrid({
  columns,
  rows,
  status,
}: {
  columns: string[];
  rows: HeatRow[];
  status: SectionStatus;
}) {
  return (
    <section
      aria-label="Model by effort"
      aria-busy={status.kind === 'loading'}
      data-component="heat-grid"
      data-dataset="models"
      class="ts-card ts-card--chart"
    >
      <span class="ts-eyebrow">Models and effort</span>
      <h2 class="ts-h2">Cost by model and effort</h2>
      {status.kind === 'ready' ? (
        <div class="ts-scroll" tabIndex={0} role="region" aria-label="Cost grid, scrolls sideways">
          <div
            class="ts-heat"
            role="table"
            aria-label="Cost in USD by model and effort"
            style={{ gridTemplateColumns: `190px repeat(${columns.length}, minmax(90px, 1fr))` }}
          >
            <div class="ts-heat__row" role="row">
              <span class="ts-heat__corner" role="columnheader">
                Model
              </span>
              {columns.map((c) => (
                <span key={c} class="ts-heat__col" role="columnheader">
                  {c}
                </span>
              ))}
            </div>
            {rows.map((r) => (
              <div key={r.name} class="ts-heat__row" role="row">
                <span class="ts-heat__model ts-mono" role="rowheader">
                  <span class="ts-dot" style={{ background: r.color }} />
                  {r.name}
                </span>
                {r.cells.map((cell, i) => (
                  <span
                    key={columns[i]}
                    role="cell"
                    class={`ts-heat__cell ts-heat__cell--${cell.level}`}
                    aria-label={cell.label}
                  >
                    {cell.text}
                  </span>
                ))}
              </div>
            ))}
          </div>
        </div>
      ) : (
        <SectionState status={status} />
      )}
      <span class="ts-note">Effort is read per turn from Claude Code and Codex. Cursor reports none of its own.</span>
    </section>
  );
}
