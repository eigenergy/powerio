# Validate caseio's PowerModels JSON against PowerModels' own parse of the .m.
# Both sides go through parse_file + make_per_unit! so PowerModels applies its
# corrections (per-unit, angle clamp) uniformly; we then deep-compare the core
# element types. Usage: julia --project=benchmarks validate_powermodels.jl case.m our.json
using PowerModels
PowerModels.silence()

ref_m, our_json = ARGS[1], ARGS[2]
ref  = PowerModels.parse_file(ref_m)
ours = PowerModels.parse_file(our_json)

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
            f in ("source_id", "index") && continue
            if !haskey(ov, f)
                push!(mismatches, "$et[$k].$f: missing in ours (ref=$rfv)")
            elseif !approx(rfv, ov[f])
                push!(mismatches, "$et[$k].$f: ref=$rfv ours=$(ov[f])")
            end
        end
        for f in keys(ov)
            f in ("source_id", "index") && continue
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
