# powerio-cli

`powerio-cli` provides the `powerio` command for format conversion, matrix
export, DC OPF bundles, PTDF/LODF exports, GridFM Parquet export, synthetic case
generation, `.pio.json` package emission, verification, and the ratatui TUI.

```
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio package tests/data/case14.m -o case14.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio
```

The workspace README has install notes and library examples:
<https://github.com/eigenergy/powerio>.
