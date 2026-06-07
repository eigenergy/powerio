# Validate caseio's PowerModels JSON against PowerModels' own parse of the .m,
# value for value over bus/branch/gen/load/shunt after per-unit normalization.
# Usage: julia --project=benchmarks validate_powermodels.jl case.m our.json
#
# validate=false skips PowerModels' correct_network_data! pass on parse. Needed
# because caseio writes unbounded limits (a pegase gen qmax=Inf) as JSON null,
# which PowerModels reads as `nothing`; correct_network_data!'s in-parse
# make_per_unit! then divides nothing/base and throws before we can restore it.
# We restore the nulls below and run make_per_unit! ourselves, which is exactly
# what the original two-step (restore, then per-unit) was written for.
#
# Cases carrying MATPOWER dclines are skipped by run_validation.sh: caseio writes
# dcline limits under MATPOWER names (mp_pmax) not PowerModels' pmaxf, and its
# mixed-model gencost export for those cases doesn't round-trip; the PSS/E,
# ExaPowerIO, and pandapower checks still cover them.
using PowerModels
PowerModels.silence()

ref_m, our_json = ARGS[1], ARGS[2]
ref  = PowerModels.parse_file(ref_m; validate=false)
ours = PowerModels.parse_file(our_json; validate=false)

# JSON has no ±Inf/NaN, so an unbounded ref value (e.g. a pegase gen qmax=Inf)
# arrives as `nothing` on our side. Accept that as a match and restore the
# value so make_per_unit! (which divides by the base) doesn't trip on nothing.
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

PowerModels.make_per_unit!(ref)
PowerModels.make_per_unit!(ours)

approx(a, b) = (a isa Number && b isa Number) ? isapprox(float(a), float(b); atol=1e-9, rtol=1e-7) : a == b

# source_id/index are bookkeeping. `transformer` is a flag PowerModels derives
# from tap/shift; caseio labels a handful of unity-tap pegase branches the other
# way, but the tap/shift/r/x/b those branches carry are compared below and match
# (and the pandapower Y_bus check agrees), so the derived label isn't compared.
const SKIP = ("source_id", "index", "transformer")

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
