import { useEffect, useRef, useState } from 'preact/hooks';
import type { Resource } from '../api/useResource';
import { rangeText } from '../format';
import { RANGES, TABS, THEMES, type Filters, type Range, type Tab, type Theme, type UrlState } from '../state';
import { EFFORTS, TOOLS, type Status } from '../types';
import { Segmented } from './Segmented';

const ALL_WORKSPACES = 'All workspaces';

export type HeaderProps = {
  state: UrlState;
  theme: Theme;
  today: string;
  status: Resource<Status>;
  onRetryStatus: () => void;
  /** Filter choices, from the repos and models in the current range. */
  repoOptions: string[];
  modelOptions: string[];
  onTheme: (theme: Theme) => void;
  onTab: (tab: Tab) => void;
  onRange: (range: Range) => void;
  onCustom: (from: string, to: string) => void;
  onFilter: (key: keyof Filters, value: string) => void;
};

/** app-header: logo, server, workspace, theme, filters, date range and tabs (the global state). */
export function AppHeader(p: HeaderProps) {
  const { state } = p;
  const [filtersOpen, setFiltersOpen] = useState(false);
  const tabsRef = useRef<HTMLElement>(null);

  // On a phone the tab row scrolls: keep the current tab in view (a link can open on the last one).
  // The row is scrolled directly: scrollIntoView would also move the browser's Tab starting point
  // to the tab (Chromium on Linux does), so Tab would skip every control above it.
  useEffect(() => {
    const nav = tabsRef.current;
    const current = nav?.querySelector('[aria-current="page"]');
    if (!nav || !current) return;
    const row = nav.getBoundingClientRect();
    const tab = current.getBoundingClientRect();
    if (tab.left < row.left) nav.scrollLeft += tab.left - row.left;
    else if (tab.right > row.right) nav.scrollLeft += tab.right - row.right;
  }, [state.tab]);

  return (
    <header data-component="app-header" data-dataset="status" class="ts-header">
      <div class="ts-header__inner">
        <div class="ts-header__top">
          <div class="ts-brand">
            <div class="ts-logo">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke-width="2" aria-hidden="true">
                <path d="M4 19V9M10 19V5M16 19v-7M22 19H2" />
              </svg>
            </div>
            <div class="ts-brand__text">
              <span class="ts-brand__name">tokensave</span>
              <ServerLine status={p.status} onRetry={p.onRetryStatus} />
            </div>
          </div>
          <div class="ts-header__controls">
            <label class="ts-field">
              Workspace
              <select
                class="ts-select"
                value={state.workspace}
                onChange={(e) => p.onFilter('workspace', e.currentTarget.value)}
              >
                {workspaceOptions(p.status, state.workspace).map((w) => (
                  <option key={w} value={w}>
                    {w}
                  </option>
                ))}
                <option value="">{ALL_WORKSPACES}</option>
              </select>
            </label>
            <Segmented label="Theme" options={THEMES} value={p.theme} onChange={p.onTheme} />
          </div>
        </div>
        <button
          type="button"
          class="ts-filterbtn"
          aria-expanded={filtersOpen}
          aria-controls="ts-filters"
          onClick={() => setFiltersOpen((open) => !open)}
        >
          Filters · {filterSummary(state)}
        </button>
        <div id="ts-filters" class={filtersOpen ? 'ts-filters is-open' : 'ts-filters'}>
          <FilterSelect
            label="Repo"
            all="All repos"
            value={state.repo}
            options={withValue(p.repoOptions, state.repo)}
            onChange={(v) => p.onFilter('repo', v)}
          />
          <FilterSelect
            label="Agent tool"
            all="All"
            value={state.tool}
            options={TOOLS}
            onChange={(v) => p.onFilter('tool', v)}
          />
          <FilterSelect
            label="Model"
            all="All"
            value={state.model}
            options={withValue(p.modelOptions, state.model)}
            onChange={(v) => p.onFilter('model', v)}
          />
          <FilterSelect
            label="Effort"
            all="All"
            value={state.effort}
            options={EFFORTS}
            onChange={(v) => p.onFilter('effort', v)}
          />
        </div>
        <div class="ts-rangebar">
          <span class="ts-rangebar__label">Date range</span>
          <Segmented label="Date range" options={RANGES} value={state.range} onChange={p.onRange} separated />
          {state.range === 'custom' && (
            <div class="ts-dates">
              <label class="ts-field">
                From
                <input
                  type="date"
                  class="ts-date"
                  value={state.from}
                  max={state.to}
                  required
                  onChange={(e) => e.currentTarget.value && p.onCustom(e.currentTarget.value, state.to)}
                />
              </label>
              <label class="ts-field">
                To
                <input
                  type="date"
                  class="ts-date"
                  value={state.to}
                  min={state.from}
                  max={p.today}
                  required
                  onChange={(e) => e.currentTarget.value && p.onCustom(state.from, e.currentTarget.value)}
                />
              </label>
            </div>
          )}
          <span class="ts-rangebar__text">{rangeText(...rangeBounds(state, p.today))}</span>
        </div>
        <nav aria-label="Dashboard sections" class="ts-tabs" ref={tabsRef}>
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              aria-current={state.tab === t.id ? 'page' : 'false'}
              onClick={() => p.onTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </div>
    </header>
  );
}

/** The first and last day the range covers. */
export function rangeBounds(state: Pick<UrlState, 'range' | 'from' | 'to'>, today: string): [string, string] {
  if (state.range === 'custom') return [state.from, state.to];
  const start = new Date(Date.parse(`${today}T00:00:00Z`) - (Number(state.range) - 1) * 86_400_000);
  return [start.toISOString().slice(0, 10), today];
}

function ServerLine({ status, onRetry }: { status: Resource<Status>; onRetry: () => void }) {
  if (status.status === 'ready') {
    return (
      <span class="ts-brand__sub">
        Dashboard · {status.data.bind}:{status.data.port}
      </span>
    );
  }
  if (status.status === 'loading') {
    return (
      <span class="ts-brand__sub" role="status">
        Dashboard · connecting…
      </span>
    );
  }
  return (
    <span class="ts-brand__sub ts-brand__sub--error" role="alert" title={status.message}>
      Dashboard · server not reachable{' '}
      <button type="button" class="ts-retry ts-retry--inline" onClick={onRetry}>
        Retry
      </button>
    </span>
  );
}

function workspaceOptions(status: Resource<Status>, current: string): string[] {
  const listed = status.status === 'ready' ? status.data.workspaces : [];
  return withValue(listed, current);
}

/** The options plus the current value, so a value from a shared link stays visible and selected. */
function withValue(options: readonly string[], value: string): string[] {
  return value && !options.includes(value) ? [...options, value] : [...options];
}

function filterSummary(s: Filters): string {
  const parts = [s.repo || 'all repos', s.tool || 'all tools'];
  if (s.model) parts.push(s.model);
  if (s.effort) parts.push(`${s.effort} effort`);
  return parts.join(', ');
}

function FilterSelect({
  label,
  all,
  value,
  options,
  onChange,
}: {
  label: string;
  all: string;
  value: string;
  options: readonly string[];
  onChange: (value: string) => void;
}) {
  return (
    <label class="ts-field">
      {label}{' '}
      <select class="ts-select ts-select--filter" value={value} onChange={(e) => onChange(e.currentTarget.value)}>
        <option value="">{all}</option>
        {options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </select>
    </label>
  );
}
