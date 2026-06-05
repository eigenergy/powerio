# Cross-tool parse benchmark: PowerModels.jl and ExaPowerIO.jl on the same
# MATPOWER files casemat is benchmarked on. Reports median time and allocations.
#
#   julia --project=benchmarks benchmarks/bench_julia.jl [case.m ...]
#
# Add the deps once:  julia --project=benchmarks -e 'using Pkg; Pkg.add(["PowerModels","ExaPowerIO","BenchmarkTools"])'
# These numbers depend on the machine and package versions; record them in
# RESULTS.md rather than committing hardcoded values.

using BenchmarkTools

const DEFAULT = ["tests/data/case2869pegase.m", "tests/data/case118.m"]

function bench(label, f, path)
    b = @benchmark $f($path) samples = 50 evals = 1
    t_ms = median(b).time / 1e6
    alloc_mb = median(b).memory / 2^20
    println(rpad(label, 24), rpad(basename(path), 24),
            rpad(string(round(t_ms, digits = 3), " ms"), 14),
            round(alloc_mb, digits = 1), " MiB")
end

paths = isempty(ARGS) ? DEFAULT : ARGS
println(rpad("tool", 24), rpad("case", 24), rpad("median", 14), "alloc")

try
    @eval using PowerModels
    for p in paths
        bench("PowerModels.parse_file", PowerModels.parse_file, p)
    end
catch e
    @warn "PowerModels not available; skipping" exception = e
end

try
    @eval using ExaPowerIO
    for p in paths
        # Entry point per ExaPowerIO's API; adjust if the package renames it.
        bench("ExaPowerIO.parse", ExaPowerIO.parse_matpower, p)
    end
catch e
    @warn "ExaPowerIO not available; skipping" exception = e
end
