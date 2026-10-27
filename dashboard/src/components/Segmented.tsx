/** segmented: a group of buttons with one pressed (theme, date range, chart or table). */
export type SegmentedProps<T extends string> = {
  label: string;
  options: readonly { id: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  /** Lines between the buttons, and a row that scrolls sideways when it doesn't fit. */
  separated?: boolean;
};

export function Segmented<T extends string>({ label, options, value, onChange, separated }: SegmentedProps<T>) {
  return (
    <div
      data-component="segmented"
      role="group"
      aria-label={label}
      class={separated ? 'ts-seg ts-seg--separated' : 'ts-seg'}
    >
      {options.map((o) => (
        <button key={o.id} type="button" aria-pressed={o.id === value} onClick={() => onChange(o.id)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}
