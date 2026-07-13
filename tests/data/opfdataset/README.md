# OPFData fixtures

`example_0.json` is one solved case-14 example from DeepMind's OPFData
`dataset_release_1` FullTop dataset. It exercises every OPFData table: buses,
generators, loads, shunts, AC lines, transformers, their link indices, and the
solved node/edge fields.

Case 14 is used because it is the smallest published example with every table,
not because the reader is specialized to that network. OPFData uses the same
feature schema for all ten published grid families (14 through 13,659 buses)
and for FullTop and N-1 data; the adapter derives every element count and link
mapping from the current document. Synthetic count-changing tests cover both
branch and generator outage shapes without vendoring another large fixture.

- Source archive: <https://storage.googleapis.com/gridopt-dataset/dataset_release_1/pglib_opf_case14_ieee_0.tar.gz>
- Archive entry: `gridopt-dataset-tmp/dataset_release_1/pglib_opf_case14_ieee/group_0/example_0.json`
- Upstream last modified: 2024-04-24
- License: Creative Commons Attribution 4.0 International, Copyright 2024
  DeepMind Technologies Limited; see the upstream
  [LICENSE](https://storage.googleapis.com/gridopt-dataset/LICENSE)
- Size: 23,115 bytes and 819 logical lines (`wc -l` reports 818 because the
  upstream file has no trailing newline)
- SHA-256: `EA86569D01C4EF2B1472E8028CE66286B6EA19A72196C46B490B18F95440029D`

The fixture is kept byte exact so the same-format source-echo test also checks
the original formatting. OPFData directories and PyTorch Geometric's derived
`.pt` caches are not fixtures or supported case inputs.
