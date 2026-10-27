import { dataOf, type ResourceHandle } from '../api/useResource';
import { TOOL_COLOR } from '../colors';
import { Callout } from '../components/Callout';
import { RowList } from '../components/RowList';
import { statusOf } from '../components/SectionState';
import { StatStrip } from '../components/StatStrip';
import { M1, ago } from '../format';
import type { Repo, Repos as ReposData } from '../types';

/** The command that indexes every repo of the workspace that isn't indexed yet. */
export const INDEX_ALL_COMMAND = 'tokensave workspace init-all --dry-run';

/** The Repos tab; its dataset is loaded by the app, which also lists the repos as filter choices. */
export function Repos({ repos, now }: { repos: ResourceHandle<ReposData>; now: number }) {
  const data = dataOf(repos);
  const list = data?.repos ?? [];
  const unindexed = list.filter((r) => !r.indexed).length;
  const ws = data?.workspace ?? null;

  return (
    <>
      <StatStrip
        eyebrow="Workspace"
        status={statusOf(repos, (d) => d.workspace === null, 'No workspace yet: repos come from tokensave’s registry.')}
        value={ws?.path ?? ''}
        note={ws ? `marked by ${ws.marker} · scan depth ${ws.scanDepth}` : ''}
        stats={[
          { label: 'Repos', value: String(list.length) },
          { label: 'Indexed', value: String(list.length - unindexed) },
          { label: 'Not indexed', value: String(unindexed), color: 'var(--warn-fg)' },
        ]}
      />
      <RowList
        dataset="repos"
        title="Repos"
        layout="table"
        status={statusOf(repos, (d) => d.repos.length === 0, 'No repos found.')}
        rows={list}
        rowKey={(r) => r.path}
        renderRow={(r) => <RepoRow repo={r} now={now} />}
      />
      {/* The callout shares the repos dataset: it appears only when there is something to fix,
          and the two sections above already show its loading and error states. */}
      {unindexed > 0 && (
        <Callout
          dataset="repos"
          title={`${unindexed} ${unindexed === 1 ? "repo isn't" : "repos aren't"} indexed, so tokensave saves nothing there`}
          command={INDEX_ALL_COMMAND}
        />
      )}
    </>
  );
}

function RepoRow({ repo, now }: { repo: Repo; now: number }) {
  return (
    <div class="ts-line">
      <span class="ts-repo__name">
        <span class="ts-who__name">{repo.name}</span>
        <span class="ts-who__sub ts-mono">{repo.path}</span>
      </span>
      <span class={`ts-badge ts-badge--${repo.indexed ? 'ok' : 'warn'} ts-repo__status`}>
        {repo.indexed ? 'indexed' : 'not indexed'}
      </span>
      <span class="ts-repo__tokens ts-mono">
        <span class="ts-repo__figure" title="Spent">
          <span class="ts-square" style={{ background: 'var(--spent)' }} />
          <span class="ts-sr">Spent </span>
          {repo.spentTokens === null ? '—' : M1(repo.spentTokens)}
        </span>
        <span class="ts-repo__figure" title="Saved by tokensave">
          <span class="ts-square" style={{ background: 'var(--saved)' }} />
          <span class="ts-sr">Saved </span>
          {repo.savedTokens === null ? '—' : M1(repo.savedTokens)}
        </span>
      </span>
      <span class="ts-repo__tools">
        {repo.tools.map((t) => (
          <span key={t} class="ts-repo__tool">
            <span class="ts-dot ts-dot--small" style={{ background: TOOL_COLOR[t] }} />
            {t}
          </span>
        ))}
      </span>
      <span class="ts-repo__last">{ago(repo.lastActivity, now)}</span>
    </div>
  );
}
