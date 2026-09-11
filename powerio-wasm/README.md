# PowerIO browser binding

This unpublished crate runs the PowerIO parser and writers on caller-supplied
bytes. `Conversion` retains one parsed module, emits selected targets, and
transfers each artifact to JavaScript. The browser worker releases the case
after conversion. No filesystem or network access is part of this binding.

Build the browser module with `npm run wasm` from `apps/converter`. The command
requires `wasm-pack` 0.15.0 and the `wasm32-unknown-unknown` Rust target.

`cargo test -p powerio-wasm` compares native facade output with binding output,
including artifact bytes and diagnostic codes. The converter workflow also
builds the actual WebAssembly module.
