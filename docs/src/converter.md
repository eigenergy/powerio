# Convert files in the browser

[Open PowerIO Convert](https://powerio.dev/convert/) to convert a file, a
folder, or a batch without installing PowerIO. Files stay on the computer.
The same Rust parsers and writers run locally through WebAssembly.

1. Choose files, drop a folder, or select ZIP projects. PowerIO identifies
   each input format, including mixed transmission and distribution cases.
2. Choose one or more output formats. Transmission defaults to MATPOWER;
   distribution defaults to PMD engineering JSON. Each case can override
   those choices.
3. Convert, review the diagnostics, and download individual outputs or a
   ZIP containing all completed outputs and a conversion report.

The examples include a MATPOWER transmission case and a small OpenDSS
feeder. The mixed example shows both in one batch.

## Projects and recovery

Choose the complete folder or ZIP for an OpenDSS, PyPSA CSV, or CGMES
project. Relative references stay within the supplied files. A project
with several possible OpenDSS entry files asks which one to open. An
incomplete project can accept missing files and retry. Formats can also
be selected explicitly when detection needs help.

Each project permits up to 4096 files and 64 MiB of expanded input. ZIP
archives have additional path and expansion checks. Batches have no fixed
case-count limit; browser memory and local storage determine capacity.
The [command line](cli-mcp.md) is useful for larger projects.

A failed case does not prevent other cases from converting. Cancel stops
the active worker and keeps completed results. Changing an input format,
entry file, or dependency removes its old outputs before conversion.

## Understand the result

Same-format emission preserves the source bytes. Cross-format emission
reports data that the target cannot represent. A warning means the output
needs review, even when a file was produced. The portal displays PowerIO's
diagnostic codes, messages, locations when available, and suggested actions.
See [Formats and fidelity](format-fidelity.md) for the supported profiles.

BMOPF outputs explicitly select **0.1.0** or the **0.2.0 proposal**. The
[BMOPF task force](https://github.com/distribution-system-opt) maintains the
collaborative distribution-system optimization work. GO Challenge 3
writing requires a complete SCUC solution. The converter does not solve a
network or synthesize missing solution data.

PowerIO IR, geographic-only data, GridFM/Parquet, matrices, and solver
operations belong in the package or terminal workflow. The portal provides
matching CLI commands for the selected input and outputs.

## Privacy, sharing, and help

The converter has no file-upload endpoint. Files, filenames, paths, and
raw diagnostics never enter analytics. Completed outputs may use temporary
browser storage. Clear all removes the session's files; abandoned temporary
files are removed on the next visit. Closing or reloading the tab discards
the queue, so download results before leaving.

Optional Umami statistics help identify formats and parser problems that
need work. They are off by default; enable them in the Privacy section.
Only activity after opt-in is shared. Statistics include standard format
names, broad outcomes, reviewed parser codes, software version, and coarse
batch-size and elapsed-time ranges. Unknown codes become `other`. Electrical
diagnostics and grid properties stay local. Duplicate problems and event
limits reduce repeated reporting.

The analytics script runs in an isolated iframe that cannot read files or
the converter page. Umami receives IP and browser connection metadata with
its requests and derives browser/device, approximate location, and visit
statistics from that metadata. See [Umami metric definitions](https://docs.umami.is/docs/metric-definitions).
Switch statistics off at any time; Do Not Track and Global
Privacy Control also disable them. Conversion works with analytics blocked.
The [public analytics policy](https://github.com/eigenergy/powerio/blob/main/apps/converter/public/analytics-policy.js)
lists every permitted event and value.

Share settings copies a link containing output preferences only. It never
includes files or filenames. Report a conversion problem prepares an
editable GitHub issue report with engine/build information, browser major
version, format names, and diagnostic codes. Review the report and add a
small, shareable example if needed. The locally downloaded conversion
report is more detailed and can contain private filenames and messages;
review it before sharing.
