# tokensave dashboard page

The page that `tokensave dashboard` serves: Preact + TypeScript, built by Vite into `dist/`.
It is translated one to one from the approved mockup (inkcasthq/inkcast
`docs/tooling/tokensave-dashboard/mockups/source/Main.dc.html`, design in
`docs/tooling/tokensave-dashboard/design.md`), following the mockup standard
(`docs/tooling/mockup-standard.md` there).

## Built files are committed

`dist/` (`index.html`, `tokens.css`, `app.js`, `app.css`) is committed, and the server embeds
every file in it at compile time. So `cargo build` and `cargo install` never need Node: Node is
needed only to change the page. CI rebuilds `dist/` from the sources and fails when the result
differs from the committed files, so the two can't drift.

After changing anything under `src/`, `public/` or `index.html`:

```sh
cd dashboard
npm ci
npm run build      # rewrites dist/; commit it with the change
```

## Where things are

| Mockup | Here |
|---|---|
| the two token tables in `renderVals()` | `public/tokens.css`: `:root[data-theme=dark\|light\|auto]`, `--cat-*`; the only file with colour values |
| `// type` comments | `src/types.ts` |
| each `// dataset` line | one module in `src/api/` (path, query, type) and `useDataset` |
| `// state` lines | `src/state.ts`: tab, range, from, to and filters in the URL; theme in browser storage |
| each `data-component` type | one component in `src/components/` |
| formatting helpers in `renderVals()` | `src/format.ts` |
| `TOOL`, `ROLE`, `MODEL` colour maps | `src/colors.ts` |
| sample data | `tests/fixtures/mockup.ts` (tests only), and its JSON in `tests/fixtures/json/` |

The page calls the API at `/api/...` with the query parameters its dataset line lists. A preset
range is sent as `range=N`; a custom range as `from` and `to`. Filters set to "all" are not sent.

## Tests

```sh
npm run typecheck   # tsc: sources, tests and the fixtures against src/types.ts
npm test            # Vitest + @testing-library/preact: formatting, state, every component and its
                    # loading, empty and error states, every tab against the fixtures, the API
                    # contract, and the contrast of every token pair the design names
npm run build
npx playwright install chromium
npm run e2e         # Playwright on the built page under the server's CSP: every tab at 390, 768,
                    # 820, 1024 and 1440 px in both themes (no sideways scroll; screenshots in
                    # test-results/screens), axe, keyboard, URL and theme behaviour
```

`UPDATE_FIXTURES=1 npm test` rewrites `tests/fixtures/json/` from the typed fixtures.

### Compare with the mockup

Text renders differently from machine to machine, so the pixel comparison runs against the
mockup rendered on the same machine. From an inkcast checkout:

```sh
scripts/mockups/check.py docs/tooling/tokensave-dashboard/mockups/source/Main.dc.html --out /tmp/mockup
```

then here:

```sh
rm -rf test-results/mockup-diff
MOCKUP_DIR=/tmp/mockup npx playwright test tests/e2e/mockup-diff.spec.ts --workers=1
```

`test-results/mockup-diff/report.md` lists, per tab, width and theme, the share of differing
pixels for the whole page and for `<main>` alone, with a diff image for each.
