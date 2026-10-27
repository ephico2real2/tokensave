import { fallbacksApi } from '../api/fallbacks';
import { mixApi } from '../api/mix';
import type { Scope } from '../api/params';
import { thinkingApi } from '../api/thinking';
import { dataOf, useDataset, type ResourceHandle } from '../api/useResource';
import { modelColors } from '../colors';
import { BarList } from '../components/BarList';
import { HeatGrid, type HeatCell } from '../components/HeatGrid';
import { RowList } from '../components/RowList';
import { statusOf } from '../components/SectionState';
import { heatUsd, modelLabel, pct, pct1, shortModel } from '../format';
import type { Fallbacks, ModelEffort } from '../types';

/** Heat levels from cost, as the mockup sets them: $10, $40 and $100. */
export function heatCell(cost: number | null, model: string, effort: string): HeatCell {
  if (cost === null) return { text: '—', level: 'none', label: `${model}, ${effort}: no turns` };
  const level = cost >= 100 ? 3 : cost >= 40 ? 2 : cost >= 10 ? 1 : 0;
  return { text: heatUsd(cost), level, label: `${model}, ${effort}: ${heatUsd(cost)}` };
}

/** The Models and effort tab; the model × effort dataset is loaded by the app (filter choices). */
export function Models({ scope, models }: { scope: Scope; models: ResourceHandle<ModelEffort> }) {
  const thinking = useDataset(thinkingApi, scope);
  const fallbacks = useDataset(fallbacksApi, scope);
  // Models keep the colour they have on the Overview: their place in the spend order.
  const colorOf = modelColors(dataOf(useDataset(mixApi, scope)));
  const grid = dataOf(models);
  const fb = dataOf(fallbacks);

  return (
    <>
      <HeatGrid
        status={statusOf(models, (m) => m.rows.length === 0, 'No turns in this range.')}
        columns={grid?.efforts ?? []}
        rows={(grid?.rows ?? []).map((r) => ({
          name: r.model,
          color: colorOf(r.model),
          cells: r.costUsd.map((c, i) => heatCell(c, r.model, grid?.efforts[i] ?? '')),
        }))}
      />
      <div class="ts-grid ts-grid--280">
        <BarList
          dataset="thinking"
          eyebrow="Thinking"
          title="Thinking share of output"
          status={statusOf(thinking, (t) => t.length === 0, 'No thinking tokens in this range.')}
          rows={(dataOf(thinking) ?? []).map((r) => ({
            name: modelLabel(r.model, r.effort),
            value: pct(r.share),
            pct: pct(r.share),
            color: colorOf(r.model),
            mono: true,
          }))}
        />
        <RowList
          dataset="fallbacks"
          eyebrow="Fallbacks"
          title="Served by a different model than requested"
          layout="boxes"
          status={statusOf(fallbacks, (f) => f.pairs.length === 0, 'Every turn was served by the model it asked for.')}
          rows={fb?.pairs ?? []}
          rowKey={(f) => `${f.requested}→${f.served}`}
          renderRow={(f) => <FallbackRow pair={f} />}
          stats={fb && [{ label: 'Fallback rate', value: pct1(fb.rate) }]}
        />
      </div>
    </>
  );
}

function FallbackRow({ pair }: { pair: Fallbacks['pairs'][number] }) {
  return (
    <div class="ts-box ts-box--row">
      <span class="ts-mono">
        {shortModel(pair.requested)} → {shortModel(pair.served)}
      </span>
      <span class="ts-muted">
        {pair.turns} {pair.turns === 1 ? 'turn' : 'turns'} · {pair.sessions} {pair.sessions === 1 ? 'session' : 'sessions'}
      </span>
    </div>
  );
}
