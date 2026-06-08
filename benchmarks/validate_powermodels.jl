# Validate powerio's PowerModels JSON against PowerModels' own parse of the .m,
# value for value over bus/branch/gen/load/shunt. The comparison kernel is shared
# with the batched driver (oracle_compare.jl); this is the standalone one-case
# wrapper (validate_oracles.jl runs every case in one process to amortize the
# PowerModels load).
#
#   julia --project=benchmarks validate_powermodels.jl case.m our.json
using PowerModels
PowerModels.silence()
include(joinpath(@__DIR__, "oracle_compare.jl"))

ref_m, our_json = ARGS[1], ARGS[2]
problems = compare_powermodels(ref_m, our_json)
if isempty(problems)
    println("MATCH: ", basename(ref_m), " — loads under validate=true; bus/branch/gen/load/shunt identical per-unit")
else
    println("MISMATCH: ", basename(ref_m), " (", length(problems), ")")
    for m in first(problems, 40)
        println("  ", m)
    end
    exit(1)
end
