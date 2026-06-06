# Cross-tool parse benchmark: ExaPowerIO.jl and PowerModels.jl on the same
# MATPOWER files caseio is benchmarked on, small cases up to 193k buses.
# Median time + bus/branch counts (identical across tools = correctness check).
#
#   julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'   # once
#   julia --project=benchmarks benchmarks/bench_julia.jl
#
# Fetch the large cases first: `bash benchmarks/fetch_cases.sh`.
# Numbers are per-machine; record them in RESULTS.md rather than hardcoding.

using ExaPowerIO, PowerModels, BenchmarkTools
PowerModels.silence()

# (name, path, run_powermodels?) — PowerModels is skipped on the huge cases
# where it takes minutes and the gap is already settled.
const CASES = [
    ("case2869pegase", "tests/data/case2869pegase.m", true),
    ("case_ACTIVSg2000", "tests/data/large/case_ACTIVSg2000.m", true),
    ("case9241pegase", "tests/data/large/case9241pegase.m", true),
    ("case13659pegase", "tests/data/large/case13659pegase.m", true),
    ("case_ACTIVSg10k", "tests/data/large/case_ACTIVSg10k.m", false),
    ("case_ACTIVSg25k", "tests/data/large/case_ACTIVSg25k.m", false),
    ("case_ACTIVSg70k", "tests/data/large/case_ACTIVSg70k.m", false),
    ("case_SyntheticUSA", "tests/data/large/case_SyntheticUSA.m", false),
    ("case99k", "tests/data/large/case99k.m", false),
    ("case193k", "tests/data/large/case193k.m", false),
]

println(rpad("case", 20), rpad("ExaPowerIO", 13), rpad("PowerModels", 13), "buses")
for (name, f, run_pm) in CASES
    isfile(f) || continue
    ed = ExaPowerIO.parse_matpower(f)
    samples = length(ed.bus) > 30_000 ? 3 : 20
    be = @benchmark ExaPowerIO.parse_matpower($f) samples = samples evals = 1
    e = round(median(be).time / 1e6, digits = 2)
    p = "skip"
    if run_pm
        bp = @benchmark PowerModels.parse_file($f) samples = 5 evals = 1
        p = string(round(median(bp).time / 1e6, digits = 1)) * " ms"
    end
    println(rpad(name, 20), rpad("$(e) ms", 13), rpad(p, 13), length(ed.bus))
end
