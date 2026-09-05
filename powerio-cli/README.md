# powerio-cli

The `powerio` command converts formats, summarizes cases, writes PowerIO IR,
exports matrices and GridFM Parquet datasets, writes DC OPF bundles and
sensitivities, generates synthetic cases, extracts and applies geographic
layers, runs the private corpus harness, and opens a terminal interface.
It reads and writes every format in the
[format table](https://github.com/eigenergy/powerio#formats).

```
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio convert case.surge.json --from surge-json --to matpower -o case.m
powerio convert goc3_case.json --from goc3-json --to matpower -o case.m
powerio convert case.xiidm --to cgmes -o case-cgmes
powerio serialize tests/data/case14.m -o case14.pio.json
powerio serialize goc3_case.json --from goc3-json -o goc3_case.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out --solver auto --drop-tolerance 1e-10
powerio
```

`powerio sensitivities --solver sparse` writes Matrix Market coordinates
through temp files, so the command does not have to hold the full sparse
PTDF/LODF output in memory.

Install notes and library examples are in the workspace README:
<https://github.com/eigenergy/powerio>.
