# Formats and Fidelity

Every transmission format crosses through `Network`. If you write a parsed file
back to the same file type, PowerIO returns the original text when the reader
kept it. If you convert to another file type, PowerIO writes the modeled power
flow data and returns warnings for fields the target cannot represent.

| format | read | write | fidelity notes |
| --- | --- | --- | --- |
| MATPOWER `.m` | yes | yes | writing back to MATPOWER is byte exact, including ignored fields and comments |
| PowerModels JSON | yes | yes | structured PowerModels data; per unit conventions are validated against PowerModels.jl |
| PSS/E `.raw` | yes | yes | v33 write path; v34/v35 fixture reads are covered by tests |
| PowerWorld `.aux` | yes | yes | text reader and writer preserve the modeled power flow core |
| PowerWorld `.pwb` | yes | no | read only binary reader; coverage is tied to sibling aux, raw, or MATPOWER files |
| PowerWorld `.pwd` | display only | no | display metadata, not a network case |
| egret JSON | yes | yes | checked against egret's own model data shape |
| pandapower JSON | yes | yes | directory independent JSON form; import validation uses pandapower |
| PyPSA CSV folder | yes | yes | static component folder; validation imports the output with PyPSA |
| PSLF EPC | yes | yes | legacy text target with warning based projection for unsupported fields |
| GridFM Parquet | yes | yes | dataset workflow in `powerio-matrix`; read side is lossy but power flow complete |

Warnings are part of the format behavior. A writer must name a lost or projected
field instead of dropping it silently. A reader that reconstructs a lossy source,
such as GridFM, parks read warnings on the returned handle so bindings can
surface them through the same warning path.

The detailed tables stay in the existing guides:

- [format fidelity](../guides/format-fidelity.html)
- [PowerWorld evidence](../guides/powerworld.html)
- [language API map](../guides/languages.html)
