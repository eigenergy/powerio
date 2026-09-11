# PowerIO Convert

A static Svelte application at <https://powerio.dev/convert/>. The native
PowerIO parser and writers run in a dedicated WebAssembly worker. No
conversion API, account, or server storage is required.

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

On powerio.dev only, an iframe with `sandbox="allow-scripts"` loads the
Umami script. Its opaque origin prevents access to the converter DOM,
selected files, browser storage, and reports. The parent sends only
allowlisted events and properties. Manual custom payloads use a fixed
page URL and title, with no query, fragment, referrer, filenames, source
content, or diagnostic messages. Umami receives normal connection
metadata. The preference switch and Do Not Track disable analytics;
blocked analytics never block conversion.

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
