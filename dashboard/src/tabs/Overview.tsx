import { dailyApi } from '../api/daily';
import { mixApi } from '../api/mix';
import type { Scope } from '../api/params';
import { summaryApi } from '../api/summary';
import { topRunsApi } from '../api/topRuns';
import { dataOf, useDataset } from '../api/useResource';
import { ROLE_COLOR, TOOL_COLOR, modelColors, tileOf } from '../colors';
import { BarList } from '../components/BarList';
import { KpiTiles, type Tile } from '../components/KpiTiles';
import { RowList } from '../components/RowList';
import { statusOf } from '../components/SectionState';
import { TimeSeries } from '../components/TimeSeries';
import { M1, M2, int, modelLabel, pct, pct1, usd } from '../format';
import type { Summary, TopRun } from '../types';

export function Overview({ scope }: { scope: Scope }) {
  const summary = useDataset(summaryApi, scope);
  const daily = useDataset(dailyApi, scope);
  const mix = useDataset(mixApi, scope);
  const top = useDataset(topRunsApi, scope);

  const summaryData = dataOf(summary);
  const mixData = dataOf(mix);
  const colorOf = modelColors(mixData);
  const mixStatus = statusOf(mix, (m) => m.byTool.length === 0 && m.byModel.length === 0, 'No turns in this range.');

  return (
    <>
      <KpiTiles
        label="Totals"
        dataset="summary"
        status={statusOf(summary, (x) => x.spentTokens === 0 && x.savedTokens === 0, 'No turns in this range.')}
        tiles={summaryData ? summaryTiles(summaryData) : []}
      />
      <TimeSeries
        status={statusOf(daily, (d) => d.every((x) => x.spentTokens === 0 && x.savedTokens === 0), 'No turns in this range.')}
        days={dataOf(daily) ?? []}
      />
      <div class="ts-grid ts-grid--280">
        <BarList
          dataset="mix"
          eyebrow="Who did the work"
          title="By agent tool"
          note="Cursor's figures come from your Cursor account, when its sync is on."
          status={mixStatus}
          rows={(mixData?.byTool ?? []).map((r) => ({
            name: r.tool,
            value: `${M1(r.tokens)} · ${pct(r.share)}`,
            pct: pct(r.share),
            color: TOOL_COLOR[r.tool],
          }))}
        />
        <BarList
          dataset="mix"
          eyebrow="On which model"
          title="By model"
          note="Models take the category colours in order of spend."
          status={mixStatus}
          rows={(mixData?.byModel ?? []).map((r) => ({
            name: r.model,
            value: `${M1(r.tokens)} · ${pct(r.share)}`,
            pct: pct(r.share),
            color: colorOf(r.model),
            mono: true,
          }))}
        />
      </div>
      <RowList
        label="Costliest runs"
        dataset="topRuns"
        eyebrow="Where it went"
        title="Costliest agent runs"
        status={statusOf(top, (t) => t.length === 0, 'No agent runs in this range.')}
        rows={dataOf(top) ?? []}
        rowKey={(r) => r.id}
        renderRow={(r) => <TopRunRow run={r} />}
      />
    </>
  );
}

function summaryTiles(s: Summary): Tile[] {
  return [
    { label: 'Tokens spent', value: M1(s.spentTokens), note: 'input, output, cache, thinking' },
    {
      label: 'Cost',
      value: usd(s.costUsd),
      note:
        s.unpricedTurns > 0
          ? `${int(s.unpricedTurns)} ${s.unpricedTurns === 1 ? 'turn has' : 'turns have'} no price and ${s.unpricedTurns === 1 ? 'is' : 'are'} left out`
          : 'from the embedded price table',
    },
    { label: 'Tokens saved', value: M1(s.savedTokens), note: 'by tokensave graph calls', color: 'var(--saved)' },
    { label: 'Saving rate', value: pct1(s.savingRate), note: 'saved ÷ (spent + saved)' },
    { label: 'Cache hit rate', value: pct(s.cacheHitRate), note: 'cache reads ÷ input' },
  ];
}

function TopRunRow({ run }: { run: TopRun }) {
  const color = ROLE_COLOR[run.role];
  return (
    <div class="ts-line">
      <span class="ts-who">
        <span class="ts-icon-tile" style={{ background: tileOf(color) }}>
          <span class="ts-dot" style={{ background: color }} />
        </span>
        <span class="ts-who__text">
          <span class="ts-who__name">{run.name}</span>
          <span class="ts-who__sub">
            {run.role} · {run.repo}
          </span>
        </span>
      </span>
      <span class="ts-line__figures ts-mono">
        <span class="ts-muted">{modelLabel(run.model, run.effort)}</span>
        <span class="ts-line__pair">
          <span>{M2(run.tokens)}</span>
          <span class="ts-line__cost">{usd(run.costUsd)}</span>
        </span>
      </span>
    </div>
  );
}
