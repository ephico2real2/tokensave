import type { Scope } from '../api/params';
import { savingsApi } from '../api/savings';
import { dataOf, useDataset } from '../api/useResource';
import { BarList } from '../components/BarList';
import { KpiTiles } from '../components/KpiTiles';
import { RowList } from '../components/RowList';
import { statusOf } from '../components/SectionState';
import { M1, int, pct } from '../format';
import type { Savings as SavingsData } from '../types';

export function Savings({ scope }: { scope: Scope }) {
  const savings = useDataset(savingsApi, scope);
  const s = dataOf(savings);

  return (
    <>
      <KpiTiles
        label="Savings totals"
        dataset="savings"
        wide
        status={statusOf(savings, (x) => x.graphCalls === 0 && x.missedTokens === 0, 'No tokensave calls in this range.')}
        tiles={
          s
            ? [
                {
                  label: 'Tokens saved',
                  value: M1(s.savedTokens),
                  note: `before ${M1(s.beforeTokens)} → after ${M1(s.afterTokens)}`,
                  color: 'var(--saved)',
                },
                { label: 'Graph calls', value: int(s.graphCalls), note: 'from the savings ledger' },
                {
                  label: 'Missed savings',
                  value: M1(s.missedTokens),
                  note: 'turns a graph call could have replaced',
                  color: 'var(--warn-fg)',
                },
              ]
            : []
        }
      />
      <div class="ts-grid ts-grid--300">
        <BarList
          dataset="savings"
          eyebrow="Savings"
          title="Saved, by tokensave tool"
          status={statusOf(savings, (d) => d.byTool.length === 0, 'No tokensave calls in this range.')}
          rows={(s?.byTool ?? []).map((r) => ({
            name: r.tool,
            value: `${M1(r.savedTokens)} · ${pct(r.share)}`,
            pct: pct(r.share),
            barColor: 'var(--saved)',
            mono: true,
          }))}
        />
        <RowList
          dataset="savings"
          eyebrow="Missed savings"
          title="Where a graph call would have fitted"
          layout="boxes"
          status={statusOf(savings, (d) => d.missed.length === 0, 'No missed savings in this range.')}
          rows={s?.missed ?? []}
          rowKey={(x) => x.kind}
          renderRow={(x) => <MissedRow missed={x} />}
        />
      </div>
    </>
  );
}

function MissedRow({ missed }: { missed: SavingsData['missed'][number] }) {
  return (
    <div class="ts-box">
      <div class="ts-box__head">
        <span class="ts-who__name">{missed.kind}</span>
        <span class="ts-mono ts-warn">
          {M1(missed.tokens)} · {int(missed.turns)} turns
        </span>
      </div>
      <span class="ts-note">
        Use instead: <span class="ts-mono ts-text">{missed.instead}</span>
      </span>
    </div>
  );
}
