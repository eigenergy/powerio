# PowerIO browser binding

This unpublished crate runs the PowerIO parser and writers on caller-supplied
bytes. `Conversion` retains one parsed module, emits selected targets, and
transfers each artifact to JavaScript. The browser worker releases the case
after conversion. No filesystem or network access is part of this binding.

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
builds the actual WebAssembly module.
