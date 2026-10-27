import { sourcesApi } from '../api/sources';
import { dataOf, useDataset } from '../api/useResource';
import { SOURCE_TONE, TOOL_COLOR } from '../colors';
import { CardGrid } from '../components/CardGrid';
import { statusOf } from '../components/SectionState';
import { ago, int } from '../format';

export function Sources({ now }: { now: number }) {
  const sources = useDataset(sourcesApi, undefined);

  return (
    <CardGrid
      eyebrow="Sources"
      title="Agent tools found on this machine"
      intro="Found by their local session data, read-only. A format tokensave doesn't recognise is reported here, never guessed."
      status={statusOf(sources, (s) => s.length === 0, 'No agent tool data found on this machine.')}
      cards={(dataOf(sources) ?? []).map((s) => ({
        key: s.tool,
        name: s.tool,
        color: TOOL_COLOR[s.tool],
        state: s.status,
        tone: SOURCE_TONE[s.status],
        path: s.path,
        figures: [
          { label: 'Turns', value: int(s.turns) },
          { label: 'Last parsed', value: s.lastParsed === null ? '—' : ago(s.lastParsed, now) },
        ],
        note: s.note,
      }))}
    />
  );
}
