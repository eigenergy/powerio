# SINCAL acquisition and corpus evidence

This branch supplies model-neutral SQLite/archive acquisition and schema
validation. The electrical readers are separate planned branches; none of
these inspection checks claims complete SINCAL parsing or fresh writer
acceptance. The [delivery plan](../../docs/design/pss-sincal.md) records the
five local PR scopes and dataset targets. The [research catalog](research-catalog.md)
records prior inspections and data rights; its deeper electrical investigations
belong to the reader branches and are not all implemented in this branch.

Run the small licensed archive and paired SimBench evidence checks with:

```sh
python3 -m unittest discover -s evals/sincal -v
python3 evals/sincal/inspect_native.py tests/data/sincal/1-LV-rural1--0-sw.sinx
cargo test -p powerio-sincal
```

The fixture's provenance and ODbL/DbCL notices live in
[tests/data/sincal](../../tests/data/sincal/README.md). The 88,876-byte archive
is unmodified; its larger unpacked database is not vendored. Original synthetic
unit tests exercise malformed inputs without copying additional native models.

The [CSIRO inventory](csiro-inventory.json) contains hashes, counts and raw
observations for 19 external CC BY 4.0 databases. It contains no model payloads.
Access acquisition is the next infrastructure implementation slice. These
historical counts are all stored rows, not effective selected-variant sizes.
External corpora remain outside the repository, and a source-code license
alone does not establish model redistribution rights.
