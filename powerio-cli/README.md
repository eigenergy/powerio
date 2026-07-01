# powerio-cli

`powerio-cli` provides the `powerio` command for format conversion, matrix
export, DC OPF bundles, PTDF/LODF exports, GridFM Parquet export, synthetic case
generation, `.pio.json` package emission, verification, and the ratatui TUI.
Transmission conversion covers MATPOWER, PSS/E, PowerWorld AUX, PSLF, PowerModels
JSON, egret JSON, pandapower JSON, PyPSA CSV folders, GOC3 JSON input, Surge
JSON, GridFM reads, and PowerIO JSON snapshots. Distribution conversion covers
OpenDSS, PMD JSON, and BMOPF JSON.

```
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio convert case.surge.json --from surge-json --to matpower -o case.m
powerio convert goc3_case.json --from goc3-json --to matpower -o case.m
powerio package tests/data/case14.m -o case14.pio.json
powerio package goc3_case.json --from goc3-json -o goc3_case.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio
```

The workspace README has install notes and library examples:
<https://github.com/eigenergy/powerio>.
