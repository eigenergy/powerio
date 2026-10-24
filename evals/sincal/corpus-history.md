# Historical CSIRO corpus audit

These two development audits predate the final compatibility reader. They are
retained here as a compact record, not a current support matrix. Full JSON
reports remain recoverable with `git show 1e95be55:evals/sincal/distribution-csiro.json`
and the corresponding `distribution-csiro-profiles.json` path. The checker
`audit_distribution.py` remains available for fresh detailed reports.

Source: [CSIRO collection](https://doi.org/10.4225/08/5631B1DF6F1A0),
Berry, Collins, Oliver and Perfumo (2015), CC BY 4.0. Source hashes and counts
are preserved in [the inventory](csiro-inventory.json).

Reader binary SHA-256: `888a136287ccaba9abcaa6655f68f4836414a863777b60e7b82a6fc88b74547a`.

Mapped counts below are component audit counts, not independently validated
networks. A successful parse is not native execution or a numerical comparison.

| Case | Elements | Default mapped | Default complete | 0 h mapped | 0 h complete |
| --- | ---: | ---: | --- | ---: | --- |
| 01 | 1033 | 545 | False | 781 | False |
| 02 | 2329 | 1055 | False | 1525 | False |
| 03 | 1084 | 776 | False | 776 | False |
| 04 | 861 | 407 | False | 575 | False |
| 05 | 1378 | 913 | False | 1369 | False |
| 06 | 218 | 163 | False | 163 | False |
| 07 | 456 | 203 | False | 283 | False |
| 08 | 309 | 265 | False | 265 | False |
| 09 | 688 | 622 | False | 688 | True |
| 10 | 124 | 99 | False | 99 | False |
| 11 | 104 | 87 | False | 87 | False |
| 12 | 215 | 188 | False | 214 | False |
| 13 | — | 0 | False | 0 | False |
| 14 | 65 | 55 | False | 55 | False |
| 15 | 102 | 89 | False | 89 | False |
| 16 | 103 | 90 | False | 90 | False |
| 17 | 142 | 130 | False | 130 | False |
| 18 | 295 | 169 | False | 169 | False |
| 19 | 33 | 1 | False | 1 | False |

For current complete-case results and limitations, use [the evidence index](README.md).
