# Validate powerio's PSS/E support against PowerModels (which reads PSS/E `.raw`).
# Generic comparator: bus/branch/gen/load and the demand/generation totals are
# strict, shunt count is informational. The comparison kernel is shared with the
# batched driver (oracle_compare.jl); this is the standalone one-case wrapper.
#
# Two uses, driven by `powerio convert`:
#   write side:  ref = case.m         test = powerio's case.raw
#   read side:   ref = real.raw       test = powerio's PowerModels JSON of real.raw
#
#   julia --project=benchmarks validate_psse.jl <ref> <test>
using PowerModels
PowerModels.silence()
include(joinpath(@__DIR__, "oracle_compare.jl"))

ref_path, test_path = ARGS[1], ARGS[2]
problems, note = compare_psse(ref_path, test_path)
name = basename(ref_path)
if isempty(problems)
    println("MATCH: $name — bus/branch/gen/load + totals identical after per-unit$note")
else
    println("MISMATCH: $name$note")
    for p in problems
        println("  ", p)
    end
    exit(1)
end
