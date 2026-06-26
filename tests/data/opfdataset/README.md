# OPFDataset Fixtures

powerio does not currently read or write OPFDataset directories. Add a schema
description and at least one small fixture here before implementing an adapter.

The adapter should map through `Network` and reuse the shared generator cost
policy. If the OPFDataset schema requires cost columns, missing PSS/E costs must
be reported in metadata or filled only when the caller selects an explicit fill
policy. Do not silently treat missing costs as real zeros.
