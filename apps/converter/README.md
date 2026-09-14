# PowerIO Convert

A static Svelte application at <https://powerio.dev/convert/>. The native
PowerIO parser and writers run in a dedicated WebAssembly worker. No
conversion API, account, or server storage is required. Rust owns parsing
and writing; plain TypeScript owns the queue, project grouping, and storage.
Svelte renders the controls, and the telemetry policy uses plain JavaScript.

## Development

Requirements: Rust with `wasm32-unknown-unknown`, wasm-pack 0.15.0, and
Node.js 22.12 or newer.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --version 0.15.0 --locked
cd apps/converter
npm ci
npm run wasm
npm run dev
```

`npm run build` produces the complete static site in `dist/`, including the
worker, WASM binary, examples, licenses, social card, and conversion landing
pages. `npm run build:ui` reuses an existing WASM build. The `/convert/` base
is intentional; previews must retain that path.

```sh
npm run check
npm test
npm run build
npx playwright install --with-deps chromium firefox webkit
npm run test:browser
```

The Docs workflow calls `converter.yml`. It publishes the tested site
artifact alongside the book, API docs, and schema archive. A failed browser
suite prevents that artifact from deploying. PR workflows build and test;
main and manual Docs runs deploy through the existing GitHub Pages job.

## Conversion and storage

`powerio-wasm` supplies the format catalog, detected model family and value
type, native diagnostics, and emitted artifacts. The frontend only groups
project files and schedules conversion. One case remains parsed at a time;
all selected targets reuse that module. A stopped worker can restart from
the compiled module without a network request.

Each project accepts at most 4096 files and 64 MiB of expanded input. ZIPs
also reject traversal, duplicate paths, encryption, nested archives, and
compression ratios above 200. Selecting multiple projects has no fixed
case-count limit. OPFS stores completed artifacts locally, with a 128 MiB
Blob fallback when OPFS is unavailable. Web Locks isolate active tabs and
allow abandoned temporary directories to be deleted on the next visit.
The queue does not persist across reloads. Clear all removes its local
artifacts. A local report can include private filenames and diagnostics.

The portal excludes PowerIO IR, geographic-only values, GridFM/Parquet,
and calculation operations. GO Challenge 3 writing requires a complete
`powerio.AcScucSolution`; network inputs cannot produce one. BMOPF outputs
use explicit 0.1.0 and 0.2.0 proposal profiles, with links to the
[BMOPF task force](https://github.com/distribution-system-opt).

## Privacy and analytics

The application CSP limits network requests to its own origin. Fonts,
examples, JavaScript, and WASM ship with the site. Conversion runs offline
after the engine loads; a fresh offline visit still requires those assets.
There is no file-upload endpoint or service worker.

Statistics are off until the visitor opts in. No analytics script or request
loads before consent. The switch persists locally, and Do Not Track or
Global Privacy Control overrides it. Opting out removes the frame and its
pending events; opting in does not upload earlier activity. Conversion
works when the analytics service is blocked or unavailable.

On powerio.dev only, an iframe with `sandbox="allow-scripts"` loads Umami.
It loads the third-party script only after a consent message from the
converter; opening the frame URL directly sends nothing. Its opaque origin prevents access to the converter DOM, selected files,
browser storage, and reports. Both sides apply the same closed vocabulary
in [`public/analytics-policy.js`](public/analytics-policy.js). Unknown
format values become `unknown`; unreviewed diagnostic codes become `other`.
There is no general text, numeric, or regular-expression escape hatch.
The reviewed codes describe parsing, format compatibility, project-file
acquisition, or browser failures, never electrical conditions or equipment.
New codes and formats require an explicit edit to this list.

Events describe engine startup, parse and conversion outcomes, selected
parser problem codes, examples, downloads, sharing, CLI discovery, and issue
report preparation. Batch sizes and elapsed times use broad buckets. The
software version has a fixed allowlist. Each operation sends at most three
distinct problem codes; each page visit sends at most 20 distinct problem
events and 100 custom events overall. Each enabled frame also sends one
fixed pageview. Identical problems are counted once per
page visit. These limits mean event totals are capped observations, not an
exact count of failed cases. No case identifiers connect the observations.
The budget and duplicate set live only in tab memory.

Custom payloads use a fixed page URL and title. Files, filenames, paths,
raw diagnostics, report text, source snippets, coordinates, topology,
equipment counts, electrical values, file hashes, exact sizes, and exact
processing times are excluded. Query strings, fragments, referrers, custom
visitor IDs, screen dimensions, and browser language are also excluded
from the application payload. Umami still receives the IP address and
browser connection metadata inherent in an HTTP request. Umami derives
browser/OS/device, approximate country/region/city, and session/visit
statistics from that metadata, as described in its
[metric definitions](https://docs.umami.is/docs/metric-definitions). This is limited
telemetry, not a promise that no information leaves the browser or a legal
certification about CEII. Sensitive models and detailed diagnostics stay
local. Detailed reports require the visitor's review before sharing.

The implementation uses Umami's documented
[custom payload API](https://docs.umami.is/docs/tracker-functions) and
[manual tracking configuration](https://docs.umami.is/docs/tracker-configuration).
Do not enable auto-tracking, session replay, or scripts in the parent page.
Issue reports start with engine/build, browser major version, format
names, and diagnostic codes; users review and edit the text before opening
GitHub. Shared links contain output settings only.

## Visual assets

The mascot reuses the repository's PowerIO SVG. `scripts/social.mjs`
regenerates the committed 1200x630 social card with Playwright Chromium.
Atkinson Hyperlegible Next is bundled under its OFL license. The MATPOWER
example retains its upstream license and source; the distribution example
is synthetic. Notices ship in `public/`.
