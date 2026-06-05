# Cross-tool parse benchmark: ExaPowerIO.jl and PowerModels.jl on the same
# MATPOWER files caseio is benchmarked on. Median time + bus/branch counts.
#
#   julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'   # once
#   julia --project=benchmarks benchmarks/bench_julia.jl [case.m ...]
#
# Numbers are per-machine; record them in RESULTS.md rather than hardcoding.

using ExaPowerIO, PowerModels, BenchmarkTools
PowerModels.silence()

const DEFAULT = [
    "tests/data/case118.m",
    "tests/data/case2869pegase.m",
    "tests/data/large/case_ACTIVSg2000.m",
    "tests/data/large/case9241pegase.m",
    "tests/data/large/case13659pegase.m",
]

paths = isempty(ARGS) ? filter(isfile, DEFAULT) : ARGS
println(rpad("case", 26), rpad("ExaPowerIO", 14), rpad("PowerModels", 14), "buses/branches")
for f in paths
    ed = ExaPowerIO.parse_matpower(f)
    be = @benchmark ExaPowerIO.parse_matpower($f) samples = 40 evals = 1
    bp = @benchmark PowerModels.parse_file($f) samples = 10 evals = 1
    e = round(median(be).time / 1e6, digits = 3)
    p = round(median(bp).time / 1e6, digits = 3)
    println(rpad(basename(f), 26), rpad("$(e) ms", 14), rpad("$(p) ms", 14),
            "$(length(ed.bus))/$(length(ed.branch))")
end
