# powerio-wasm

Browser bindings for [PowerIO](https://github.com/eigenergy/powerio). The
package runs the PowerIO parser and writers on bytes the caller supplies, in
the browser, with no filesystem or network access. It is the engine of the
[PowerIO converter](https://eigenergy.github.io/powerio/convert/), and each
release publishes it to npm at the same version as the `powerio` Python wheel
and crates.

```sh
npm install powerio-wasm
```

## Usage

The package is the `wasm-pack --target web` build: an ES module whose default
export loads the WebAssembly binary. Call it once before anything else. Every
method that reports a result returns a JSON string.

```js
import init, { capabilities, Conversion } from 'powerio-wasm';

await init();

// The formats this build reads and writes, and the PowerIO version.
const { version, formats } = JSON.parse(capabilities());

const conversion = new Conversion('case14');
const bytes = new Uint8Array(await file.arrayBuffer());
let result = JSON.parse(conversion.add_file('case14.m', bytes));
if (!result.ok) throw new Error(result.diagnostics[0].message);

// Detect the format from the primary file, or name a format token.
result = JSON.parse(conversion.inspect('case14.m', undefined));
// { ok, format, family, valueType, diagnostics }

result = JSON.parse(conversion.emit('psse', 'case14.raw'));
// { ok, layout: "file" | "directory", fidelity, diagnostics }
for (let i = 0; i < conversion.artifact_count(); i++) {
  const path = conversion.artifact_name(i);
  const output = conversion.take_artifact(i); // Uint8Array
}

conversion.free();
```

- `capabilities()` lists each format with its `token`, `label`, `family`,
  `canRead`, `canEmit`, `isDirectory`, and `extension`.
- `new Conversion(name)` holds one case. `add_file(path, bytes)` adds one
  file under its project path; a multi-file project (a CGMES or PyPSA
  directory, for example) adds every file with its relative path. A project
  is limited to 4096 files and 64 MiB.
- `inspect(primary, format)` parses the case and keeps the parsed module.
  `discover()` lists the candidate cases in a project of overlapping inputs
  without parsing them, and `inspect_selection(json)` parses one of those
  candidates.
- `emit(format, name)` writes the parsed case in a target format and keeps
  the output files. `name` is the output file name, or the directory name
  for a directory format (`isDirectory`); `artifact_count`, `artifact_name`, and `take_artifact`
  hand them over. `take_artifact` moves the bytes out, so each artifact is
  taken once.
- `release_result()` drops the parsed module and outputs and keeps the input
  files; `free()` releases everything.
- A failure returns `{ "ok": false, "diagnostics": [...] }`. Each diagnostic
  has a stable `code`, a `severity`, a `message`, and, where PowerIO states
  them, a `target`, a `suggestedAction`, and source `spans`.

Large cases parse best in a Web Worker, as the converter does, so the page
stays responsive; pass the `.wasm` URL to `init({ module_or_path })` when a
bundler serves it from another path.

## Building from source

Build the browser module with `npm run wasm` from `apps/converter`. The command
requires `wasm-pack` 0.15.0, the `wasm32-unknown-unknown` Rust target, and a
Clang/LLVM C compiler with the WebAssembly backend for SQLite. Verify that
`clang --print-targets` lists `wasm32`; when using a separate LLVM installation,
set `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` to its `clang`
and `llvm-ar`. Apple's system Clang may not include that backend.

SQLite comes from the facade's default-on `sincal` feature. This binding keeps
the default so the converter reads SINCAL projects in the browser; a binding
that does not need them depends on `powerio` with `default-features = false`
and builds with no C compiler at all.

`cargo test -p powerio-wasm` compares native facade output with binding output,
including artifact bytes and diagnostic codes. The converter workflow also
builds the actual WebAssembly module, and the npm workflow publishes the same
build.

## License

MIT OR Apache-2.0.
