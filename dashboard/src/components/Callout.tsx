/** callout: a warning or tip, with the exact command that fixes it. */
export function Callout({ dataset, title, command }: { dataset: string; title: string; command?: string }) {
  return (
    <section aria-label={title} data-component="callout" data-dataset={dataset} class="ts-callout">
      <h2 class="ts-callout__title">{title}</h2>
      {command && (
        <code class="ts-callout__code ts-mono" tabIndex={0}>
          {command}
        </code>
      )}
    </section>
  );
}
