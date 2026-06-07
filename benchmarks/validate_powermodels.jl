# Validate caseio's PowerModels JSON against PowerModels' own parse of the .m,
# value for value over bus/branch/gen/load/shunt.
# Usage: julia --project=benchmarks validate_powermodels.jl case.m our.json
#
# Both sides parse with PowerModels' default validate=true. caseio writes
# idiomatic per_unit=true JSON (the same form PowerModels exports), so parse_file
# reads it without rerunning make_per_unit! and the .m reference is per-unitized
# by the same pass — both land in per unit and compare directly. dcline cases work
# too: caseio derives PowerModels' per-end bounds, so correct_dclines! is happy.
using PowerModels
PowerModels.silence()

ref_m, our_json = ARGS[1], ARGS[2]
ref  = PowerModels.parse_file(ref_m)
ours = PowerModels.parse_file(our_json)

# JSON has no ±Inf/NaN, so an unbounded ref value (e.g. a pegase gen qmax=Inf)
# arrives as `nothing` on our side. Restore it from the reference so the two
# compare equal (both mean unbounded).
for et in ["bus", "branch", "gen", "load", "shunt"]
    haskey(ref, et) || continue
    for (k, ov) in get(ours, et, Dict())
        rv = get(ref[et], k, nothing)
        rv === nothing && continue
        for (f, val) in ov
            if val === nothing && haskey(rv, f) && rv[f] isa Number && !isfinite(rv[f])
                ov[f] = rv[f]
            end
        end
    end
end

approx(a, b) = (a isa Number && b isa Number) ? isapprox(float(a), float(b); atol=1e-9, rtol=1e-7) : a == b

# source_id/index are bookkeeping, not network data.
const SKIP = ("source_id", "index")

mismatches = String[]
for et in ["bus", "branch", "gen", "load", "shunt"]
    r = get(ref, et, Dict()); o = get(ours, et, Dict())
    if length(r) != length(o)
        push!(mismatches, "$et: count $(length(r)) (ref) vs $(length(o)) (ours)")
        continue
    end
    for (k, rv) in r
        haskey(o, k) || (push!(mismatches, "$et[$k]: missing in ours"); continue)
        ov = o[k]
        for (f, rfv) in rv
            f in SKIP && continue
            if !haskey(ov, f)
                push!(mismatches, "$et[$k].$f: missing in ours (ref=$rfv)")
            elseif !approx(rfv, ov[f])
                push!(mismatches, "$et[$k].$f: ref=$rfv ours=$(ov[f])")
            end
        end
        for f in keys(ov)
            f in SKIP && continue
            haskey(rv, f) || push!(mismatches, "$et[$k].$f: extra in ours (=$(ov[f]))")
        end
    end
end

if isempty(mismatches)
    println("MATCH: ", basename(ref_m), " — bus/branch/gen/load/shunt identical after per-unit")
else
    println("MISMATCH: ", basename(ref_m), " (", length(mismatches), ")")
    for m in first(mismatches, 40); println("  ", m); end
    exit(1)
end
