import type { Scope } from '../api/params';
import { rolesApi } from '../api/roles';
import { runsApi } from '../api/runs';
import { dataOf, useDataset } from '../api/useResource';
import { ROLE_COLOR, RUN_TONE, TOOL_COLOR, tileOf } from '../colors';
import { BarList } from '../components/BarList';
import { RowList } from '../components/RowList';
import { statusOf } from '../components/SectionState';
import { K, modelLabel, pct, usd } from '../format';
import type { Run } from '../types';

export function Agents({ scope }: { scope: Scope }) {
  const runs = useDataset(runsApi, scope);
  const roles = useDataset(rolesApi, scope);
  const roleData = dataOf(roles);

  return (
    <div class="ts-split">
      <RowList
        class="ts-split__main"
        label="Sessions and subagents"
        dataset="runs"
        eyebrow="Agent runs"
        title="Sessions and the agents they started"
        note="Roles come from each subagent's own record (type and description); nothing is read from prompts."
        status={statusOf(runs, (r) => r.length === 0, 'No agent runs in this range.')}
        rows={nestRuns(dataOf(runs) ?? [])}
        rowKey={(r) => r.id}
        renderRow={(r) => <RunRow run={r} />}
      />
      <BarList
        as="aside"
        label="Cost by role"
        dataset="roles"
        eyebrow="By role"
        title="Cost by role"
        status={statusOf(roles, (r) => r.roles.length === 0, 'No agent runs in this range.')}
        rows={(roleData?.roles ?? []).map((r) => ({
          name: r.role,
          value: `${usd(r.costUsd)} · ${pct(r.share)}`,
          pct: pct(r.share),
          color: ROLE_COLOR[r.role],
        }))}
        stats={
          roleData && [
            { label: 'Subagent share of spend', value: pct(roleData.subagentShare) },
            { label: "Runs that didn't finish", value: String(roleData.unfinishedRuns) },
          ]
        }
      />
    </div>
  );
}

/**
 * Each session followed by the subagents it started, in the server's order. A subagent whose
 * session is not in the list (outside the range or filters) is shown at the top level.
 */
export function nestRuns(runs: Run[]): Run[] {
  const ids = new Set(runs.map((r) => r.id));
  const children = new Map<string, Run[]>();
  for (const r of runs) {
    if (r.parentId !== null && ids.has(r.parentId)) {
      children.set(r.parentId, [...(children.get(r.parentId) ?? []), r]);
    }
  }
  const out: Run[] = [];
  const visit = (r: Run) => {
    out.push(r);
    (children.get(r.id) ?? []).forEach(visit);
  };
  runs.filter((r) => r.parentId === null || !ids.has(r.parentId)).forEach(visit);
  return out;
}

function RunRow({ run }: { run: Run }) {
  const isTop = run.parentId === null;
  const color = isTop ? TOOL_COLOR[run.tool] : ROLE_COLOR[run.role];
  return (
    <div class={isTop ? 'ts-line ts-line--run' : 'ts-line ts-line--run ts-line--child'}>
      <span class="ts-who ts-who--wide">
        <span class="ts-icon-tile" style={{ background: tileOf(color) }}>
          <span class="ts-dot" style={{ background: color }} />
        </span>
        <span class="ts-who__text">
          <span class={isTop ? 'ts-who__name ts-who__name--top' : 'ts-who__name ts-who__name--child'}>{run.name}</span>
          <span class="ts-who__sub">
            {run.role} · {run.tool}
          </span>
        </span>
      </span>
      <span class="ts-line__figures ts-line__figures--run ts-mono">
        <span class="ts-muted">{modelLabel(run.model, run.effort)}</span>
        <span class="ts-muted">
          {K(run.inputTokens)} / {K(run.outputTokens)}
        </span>
        <span class="ts-line__pair ts-line__pair--state">
          <span class="ts-line__cost ts-line__cost--run">{usd(run.costUsd)}</span>
          <span class={`ts-badge ts-badge--${RUN_TONE[run.state]} ts-sans`}>{run.state}</span>
        </span>
      </span>
    </div>
  );
}
