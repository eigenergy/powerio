# Export a case to PowerModels JSON the way PowerModels itself writes it
# (per_unit=true). Used by run_validation.sh to test caseio's PowerModels JSON
# *reader* against real PowerModels output: PowerModels writes the JSON, caseio
# reads it and re-emits, and the two are compared.
#
#   julia --project=benchmarks pm_export.jl <case.m> <out.json>
using PowerModels
PowerModels.silence()
PowerModels.export_file(ARGS[2], PowerModels.parse_file(ARGS[1]))
