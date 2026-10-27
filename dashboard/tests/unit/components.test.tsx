import { cleanup, fireEvent, render, screen } from '@testing-library/preact';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { BarList } from '../../src/components/BarList';
import { Callout } from '../../src/components/Callout';
import { CardGrid } from '../../src/components/CardGrid';
import { HeatGrid } from '../../src/components/HeatGrid';
import { KpiTiles } from '../../src/components/KpiTiles';
import { RowList } from '../../src/components/RowList';
import { Segmented } from '../../src/components/Segmented';
import type { SectionStatus } from '../../src/components/SectionState';
import { StatStrip } from '../../src/components/StatStrip';
import { TimeSeries } from '../../src/components/TimeSeries';
import { daily } from '../fixtures/mockup';

afterEach(cleanup);

const onRetry = vi.fn();
const STATES: [string, SectionStatus][] = [
  ['loading', { kind: 'loading' }],
  ['empty', { kind: 'empty', message: 'Nothing in this range.' }],
  ['error', { kind: 'error', message: 'GET /api/x failed: 500', onRetry }],
];

/** Each section type draws each state inside its own section, and Retry reloads. */
function expectState(section: HTMLElement, name: string) {
  if (name === 'loading') {
    expect(section.getAttribute('aria-busy')).toBe('true');
    expect(section.textContent).toContain('Loading…');
  } else if (name === 'empty') {
    expect(section.textContent).toContain('Nothing in this range.');
  } else {
    expect(section.querySelector('[role="alert"]')?.textContent).toContain('GET /api/x failed: 500');
    onRetry.mockClear();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  }
}

const section = (type: string) => document.querySelector<HTMLElement>(`[data-component="${type}"]`)!;

describe.each(STATES)('%s state', (name, status) => {
  it('kpi-tiles', () => {
    render(<KpiTiles label="Totals" dataset="summary" tiles={[]} status={status} />);
    expectState(section('kpi-tiles'), name);
  });
  it('time-series', () => {
    render(<TimeSeries days={[]} status={status} />);
    expectState(section('time-series'), name);
  });
  it('bar-list', () => {
    render(<BarList dataset="mix" eyebrow="E" title="T" rows={[]} status={status} />);
    expectState(section('bar-list'), name);
  });
  it('row-list', () => {
    render(<RowList dataset="runs" title="T" rows={[]} rowKey={String} renderRow={String} status={status} />);
    expectState(section('row-list'), name);
  });
  it('heat-grid', () => {
    render(<HeatGrid columns={[]} rows={[]} status={status} />);
    expectState(section('heat-grid'), name);
  });
  it('card-grid', () => {
    render(<CardGrid eyebrow="E" title="T" intro="I" cards={[]} status={status} />);
    expectState(section('card-grid'), name);
  });
  it('stat-strip', () => {
    render(<StatStrip eyebrow="Workspace" value="" note="" stats={[]} status={status} />);
    expectState(section('stat-strip'), name);
  });
});

describe('ready state', () => {
  it('time-series: one day group per day, values in tooltips and in the table view', () => {
    const days = daily('2026-10-26', 30);
    render(<TimeSeries days={days} status={{ kind: 'ready' }} />);
    const groups = document.querySelectorAll('.ts-plot__day');
    expect(groups).toHaveLength(30);
    expect(groups[29]?.getAttribute('title')).toMatch(/^Oct 26: spent \d+\.\d\dM, saved \d+\.\d\dM$/);
    // The busiest day's spent bar is the full 184 px plot.
    const heights = [...document.querySelectorAll<HTMLElement>('.ts-plot__bar')].map((b) => parseInt(b.style.height, 10));
    expect(Math.max(...heights)).toBe(184);
    expect(screen.getByText('Sep 27')).toBeTruthy();

    const toggle = screen.getByRole('button', { name: 'Table' });
    expect(toggle.getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-pressed')).toBe('true');
    expect(screen.getAllByRole('row')).toHaveLength(31);
    expect(screen.getByRole('rowheader', { name: 'Oct 26' })).toBeTruthy();
  });

  it('segmented: one button pressed, a click picks', () => {
    const onChange = vi.fn();
    const options = [
      { id: 'a', label: 'A' },
      { id: 'b', label: 'B' },
    ] as const;
    render(<Segmented label="Pick" options={options} value="a" onChange={onChange} />);
    expect(screen.getByRole('group', { name: 'Pick' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'A' }).getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByRole('button', { name: 'B' }).getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(screen.getByRole('button', { name: 'B' }));
    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('callout: the title and the exact command', () => {
    render(<Callout dataset="repos" title="2 repos aren't indexed" command="tokensave workspace init-all --dry-run" />);
    expect(screen.getByRole('heading', { name: "2 repos aren't indexed" })).toBeTruthy();
    expect(section('callout').querySelector('code')?.textContent).toBe('tokensave workspace init-all --dry-run');
  });
});
