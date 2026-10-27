import { tileOf, type Tone } from '../colors';
import { SectionState, type SectionStatus } from './SectionState';

export type Card = {
  key: string;
  name: string;
  color: string;
  state: string;
  tone: Tone;
  path: string;
  figures: { label: string; value: string }[];
  note: string;
};

/** card-grid: cards with an icon tile, a status badge and details; a heading above them. */
export function CardGrid({
  eyebrow,
  title,
  intro,
  cards,
  status,
}: {
  eyebrow: string;
  title: string;
  intro: string;
  cards: Card[];
  status: SectionStatus;
}) {
  return (
    <>
      <div class="ts-intro">
        <span class="ts-eyebrow">{eyebrow}</span>
        <h2 class="ts-h2 ts-h2--large">{title}</h2>
        <span class="ts-intro__text">{intro}</span>
      </div>
      <section
        aria-label={title}
        aria-busy={status.kind === 'loading'}
        data-component="card-grid"
        data-dataset="sources"
        class="ts-cards"
      >
        {status.kind === 'ready' ? (
          cards.map((c) => (
            <div key={c.key} class="ts-card ts-card--source">
              <div class="ts-source__head">
                <span class="ts-source__name">
                  <span class="ts-icon-tile ts-icon-tile--large" style={{ background: tileOf(c.color) }}>
                    <span class="ts-dot ts-dot--large" style={{ background: c.color }} />
                  </span>
                  {c.name}
                </span>
                <span class={`ts-badge ts-badge--${c.tone}`}>{c.state}</span>
              </div>
              <span class="ts-source__path ts-mono">{c.path}</span>
              <div class="ts-figures">
                {c.figures.map((f) => (
                  <div key={f.label} class="ts-figure">
                    <span class="ts-figure__label">{f.label}</span>
                    <span class="ts-mono">{f.value}</span>
                  </div>
                ))}
              </div>
              <span class="ts-note">{c.note}</span>
            </div>
          ))
        ) : (
          <div class="ts-card">
            <SectionState status={status} />
          </div>
        )}
      </section>
    </>
  );
}
