# Cross-tool parse benchmark in one process: caseio (via its C ABI), ExaPowerIO.jl,
# and PowerModels.jl on the same MATPOWER files, small cases up to 193k buses.
# Median wall time per tool + the bus count each returns. Measuring all three
# under the same BenchmarkTools harness is the apples-to-apples comparison;
# caseio used to be timed by a separate Rust example and pasted in by hand.
#
#   cargo build --release -p caseio-capi                           # once
#   julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'   # once
#   julia --project=benchmarks benchmarks/bench_julia.jl
#
# Fetch the large cases first: `bash benchmarks/fetch_cases.sh`.
# Numbers are per-machine; record them in RESULTS.md rather than hardcoding.
#
# Note on the bus count: ExaPowerIO defaults to `filtered=true`, which drops
# isolated (type-4) buses, so on cases that have them its count is below caseio's
# full, lossless count. That is a parse-policy difference, not a discrepancy; the
# value-level cross-checks live in validate_exapowerio.jl / validate_pandapower.py.

using ExaPowerIO, PowerModels, BenchmarkTools
PowerModels.silence()
include(joinpath(@__DIR__, "caseio_ffi.jl"))

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

ms(b) = round(median(b).time / 1e6, digits = 2)

println(rpad("case", 20), rpad("caseio", 13), rpad("ExaPowerIO", 13),
        rpad("PowerModels", 13), "buses (caseio / ExaPowerIO)")
for (name, f, run_pm) in CASES
    isfile(f) || continue

    # caseio through the C ABI. Time only the parse (read + build the model) and
    # free in an untimed teardown — matching ExaPowerIO/PowerModels, whose returned
    # data is GC'd outside the @benchmark sample rather than freed inside it. The
    # handle reaches teardown through a Ref, so no sample leaks.
    h = cio_parse(f); nbuses = cio_n_buses(h); cio_free(h)
    samples = nbuses > 30_000 ? 5 : 30
    href = Ref{Ptr{Cvoid}}(C_NULL)
    bc = @benchmark $href[] = cio_parse($f) teardown = (cio_free($href[])) samples = samples evals = 1
    c = ms(bc)

    ed = ExaPowerIO.parse_matpower(f)
    be = @benchmark ExaPowerIO.parse_matpower($f) samples = samples evals = 1
    e = ms(be)

    p = "skip"
    if run_pm
        bp = @benchmark PowerModels.parse_file($f) samples = 5 evals = 1
        p = string(round(median(bp).time / 1e6, digits = 1)) * " ms"
    end

    count = nbuses == length(ed.bus) ? string(nbuses) : "$nbuses / $(length(ed.bus))"
    println(rpad(name, 20), rpad("$(c) ms", 13), rpad("$(e) ms", 13),
            rpad(p, 13), count)
end
