# powerio-diag

The diagnostic vocabulary every PowerIO crate reports through: the dotted
diagnostic code and its grammar, the severity ladder, the stage family the
first segment of a code names, the coarse error categories a binding maps onto
its own taxonomy, the registry entry a code is declared with, and the renderer
that turns a finding into one `CODE: message` line.

It is a leaf: serde, serde_json, and an optional schemars derive, nothing else.
The transmission model, the distribution model, and the `.pio.json` document
model are peers above it and share this one record, so a finding crosses
between them without a translation step.

```rust
use powerio_diag::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic, render_line};

let d = StructuredDiagnostic::new(
    "EMIT.PSSE.FIELD_DROPPED",
    DiagnosticSeverity::Warning,
    "generator cost curves have no PSS/E record and are dropped",
);
assert_eq!(d.stage(), Some(DiagnosticStage::Emit));
assert_eq!(
    render_line(&d),
    "EMIT.PSSE.FIELD_DROPPED: generator cost curves have no PSS/E record and are dropped"
);
```

Part of [PowerIO](https://github.com/eigenergy/powerio). Licensed MIT or
Apache-2.0.
