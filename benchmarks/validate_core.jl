# Generic PowerModels core comparator, batched. Loads one reference and compares
# the electrical core of each test file to it: bus/branch/gen/load counts and the
# demand/generation/shunt totals after make_per_unit!. Used by validate_matrix.sh
# to check every reader->writer output that PowerModels can read (MATPOWER,
# PowerModels JSON, PSS/E, and PowerWorld via a powerio .aux -> JSON bridge)
# against the ground-truth MATPOWER case, in a single Julia process.
#
#   julia --project=benchmarks validate_core.jl <ref> <test1> [test2 ...]
#
# Prints "OK <test>" or "FAIL <test>: ..." per file; exits nonzero if any fail.
using PowerModels
PowerModels.silence()

ref = PowerModels.parse_file(ARGS[1])
# Inf survives per-unit unchanged, so per-unit the reference up front; each test
# then restores its JSON-null (unbounded) fields from it before its own per-unit.
PowerModels.make_per_unit!(ref)

cnt(d, et) = length(get(d, et, Dict()))
total(d, et, f) = round(sum(e[f] for e in values(get(d, et, Dict())); init = 0.0), digits = 4)

function restore_inf!(test)
    for et in ["bus", "branch", "gen", "load", "shunt"]
        haskey(ref, et) || continue
        for (k, tv) in get(test, et, Dict())
            rv = get(ref[et], k, nothing)
            rv === nothing && continue
            for (f, val) in tv
                if val === nothing && haskey(rv, f) && rv[f] isa Number && !isfinite(rv[f])
                    tv[f] = rv[f]
                end
            end
        end
    end
end

fails = 0
for test_path in ARGS[2:end]
    local test
    try
        test = PowerModels.parse_file(test_path)
    catch e
        println("FAIL $test_path: parse error $e")
        global fails += 1
        continue
    end
    restore_inf!(test)
    PowerModels.make_per_unit!(test)

    problems = String[]
    for et in ["bus", "branch", "gen", "load"]
        if cnt(ref, et) != cnt(test, et)
            push!(problems, "$et count ref=$(cnt(ref, et)) test=$(cnt(test, et))")
        end
    end
    # Totals are checked unconditionally, including shunt gs/bs: a dropped or
    # mis-scaled element shows up in the sum regardless of how each oracle buckets
    # elements into per-bus entries, so this catches a loss even when the counts
    # legitimately differ.
    for (et, f) in [("load", "pd"), ("load", "qd"), ("gen", "pg"), ("shunt", "gs"), ("shunt", "bs")]
        r, t = total(ref, et, f), total(test, et, f)
        isapprox(r, t; atol = 1e-6, rtol = 1e-6) || push!(problems, "Σ$et.$f ref=$r test=$t")
    end

    if isempty(problems)
        println("OK $test_path")
    else
        println("FAIL $test_path: ", join(problems, "; "))
        global fails += 1
    end
end

exit(fails == 0 ? 0 : 1)
