import { useState } from 'preact/hooks';
import { M2, shortDate, ticks } from '../format';
import type { Day } from '../types';
import { SectionState, type SectionStatus } from './SectionState';

/** Bar height for the busiest day, in px (the plot is 200 px with 8 px headroom, as in the mockup). */
const PLOT_PX = 184;

/**
 * time-series: spent and saved bars per day, a legend and date ticks. Each day has its values
 * in a tooltip, and the Table button swaps the chart for a table of every value. The chart scrolls inside its card
 * when the range has more days than fit.
 */
export function TimeSeries({ days, status }: { days: Day[]; status: SectionStatus }) {
  const [asTable, setAsTable] = useState(false);
  const peak = Math.max(0, ...days.map((d) => Math.max(d.spentTokens, d.savedTokens)));
  const height = (tokens: number) => `${peak > 0 ? Math.round((tokens / peak) * PLOT_PX) : 0}px`;
  const minWidth = `${Math.max(640, days.length * 14)}px`;
  const ready = status.kind === 'ready';

  return (
    <section
      aria-label="Spent against saved"
      aria-busy={status.kind === 'loading'}
      data-component="time-series"
      data-dataset="days"
      class="ts-card ts-card--chart"
    >
      <div class="ts-chart-head">
        <div class="ts-chart-head__title">
          <span class="ts-eyebrow">Spend and savings</span>
          <h2 class="ts-h2">Tokens spent and saved, per day</h2>
        </div>
        <div class="ts-legend">
          <span class="ts-legend__item">
            <span class="ts-swatch" style={{ background: 'var(--spent)' }} />
            Spent
          </span>
          <span class="ts-legend__item">
            <span class="ts-swatch" style={{ background: 'var(--saved)' }} />
            Saved by tokensave
          </span>
          {ready && (
            <button type="button" class="ts-toggle" aria-pressed={asTable} onClick={() => setAsTable((t) => !t)}>
              Table
            </button>
          )}
        </div>
      </div>
      {!ready && <SectionState status={status} />}
      {ready && !asTable && (
        <div class="ts-scroll" tabIndex={0} role="region" aria-label="Daily chart, scrolls sideways">
          <div class="ts-plot" style={{ minWidth }}>
            {days.map((d) => (
              <div
                key={d.date}
                class="ts-plot__day"
                title={`${shortDate(d.date)}: spent ${M2(d.spentTokens)}, saved ${M2(d.savedTokens)}`}
              >
                <div class="ts-plot__bar" style={{ background: 'var(--spent)', height: height(d.spentTokens) }} />
                <div class="ts-plot__bar" style={{ background: 'var(--saved)', height: height(d.savedTokens) }} />
              </div>
            ))}
          </div>
          <div class="ts-ticks" style={{ minWidth }} aria-hidden="true">
            {ticks(days.map((d) => d.date)).map((t, i) => (
              <span key={i}>{t}</span>
            ))}
          </div>
        </div>
      )}
      {ready && asTable && (
        <div class="ts-table-wrap" tabIndex={0} role="region" aria-label="Daily values">
          <table class="ts-table">
            <thead>
              <tr>
                <th scope="col">Day</th>
                <th scope="col">Spent</th>
                <th scope="col">Saved by tokensave</th>
              </tr>
            </thead>
            <tbody>
              {days.map((d) => (
                <tr key={d.date}>
                  <th scope="row">{shortDate(d.date)}</th>
                  <td>{M2(d.spentTokens)}</td>
                  <td>{M2(d.savedTokens)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
