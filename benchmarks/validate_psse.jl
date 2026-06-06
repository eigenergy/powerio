# Validate caseio's PSS/E support against PowerModels (which reads PSS/E `.raw`).
# Generic comparator: parse two PowerModels-readable files, make_per_unit both,
# and compare the electrical core. bus/branch/gen/load and the demand/generation
# totals are strict; shunt count is informational (caseio models fixed shunts but
# not PSS/E switched shunts).
#
# Two uses, driven by `casemat convert` (see the loop in the PR's validation run):
#   write side:  ref = case.m         test = caseio's case.raw
#   read side:   ref = real.raw       test = caseio's PowerModels JSON of real.raw
#
#   julia --project=benchmarks benchmarks/validate_psse.jl <ref> <test>
using PowerModels
PowerModels.silence()

ref_path, test_path = ARGS[1], ARGS[2]
ref  = PowerModels.parse_file(ref_path)
test = PowerModels.parse_file(test_path)

# JSON can't carry ±Inf; an unbounded value arrives as `nothing`. Restore it from
# the reference so make_per_unit! (which divides by the base) doesn't trip.
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

PowerModels.make_per_unit!(ref)
PowerModels.make_per_unit!(test)

total(d, et, f) = round(sum(e[f] for e in values(get(d, et, Dict())); init = 0.0), digits = 4)
count(d, et) = length(get(d, et, Dict()))

problems = String[]
for et in ["bus", "branch", "gen", "load"]
    if count(ref, et) != count(test, et)
        push!(problems, "$et count: ref=$(count(ref, et)) test=$(count(test, et))")
    end
end
for (et, f) in [("load", "pd"), ("load", "qd"), ("gen", "pg")]
    r, t = total(ref, et, f), total(test, et, f)
    if !isapprox(r, t; atol = 1e-6, rtol = 1e-6)
        push!(problems, "Σ$et.$f: ref=$r test=$t")
    end
end

name = basename(ref_path)
sref, stest = count(ref, "shunt"), count(test, "shunt")
shunt_note = sref == stest ? "" : "  (shunt: ref=$sref test=$stest — switched shunts not modeled)"

if isempty(problems)
    println("MATCH: $name — bus/branch/gen/load + totals identical after per-unit$shunt_note")
else
    println("MISMATCH: $name$shunt_note")
    for p in problems
        println("  ", p)
    end
    exit(1)
end
