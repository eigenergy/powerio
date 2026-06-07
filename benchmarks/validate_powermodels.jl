# Validate caseio's PowerModels JSON against PowerModels' own parse of the .m,
# value for value over bus/branch/gen/load/shunt.
# Usage: julia --project=benchmarks validate_powermodels.jl case.m our.json
#
# Two checks:
#  1. Consumability — caseio writes idiomatic per_unit=true JSON, so PowerModels'
#     default parse_file (validate=true, which runs correct_network_data! and the
#     dcline correction) must load it without error. This is the interop property
#     that motivated emitting per_unit=true.
#  2. Value-for-value — parse both sides with validate=false and per-unitize them
#     explicitly. validate=false matters: correct_network_data! clamps angmin/angmax
#     to ±60° (default_pad) and normalizes branch direction / thermal limits, which
#     would rewrite BOTH sides into agreement and hide a scaling bug. make_per_unit!
#     is a no-op on data already flagged per_unit=true (caseio's JSON), so it only
#     per-unitizes the native .m reference.
using PowerModels
PowerModels.silence()

ref_m, our_json = ARGS[1], ARGS[2]

# 1. Consumability: the caseio JSON must load under PowerModels' default validate=true.
try
    PowerModels.parse_file(our_json)
catch e
    println("MISMATCH: ", basename(ref_m), " — caseio JSON rejected by PowerModels validate=true: ", e)
    exit(1)
end

# 2. Strict comparison on un-normalized, explicitly per-unitized data.
ref  = PowerModels.parse_file(ref_m; validate=false)
ours = PowerModels.parse_file(our_json; validate=false)
PowerModels.make_per_unit!(ref)
PowerModels.make_per_unit!(ours)

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
    println("MATCH: ", basename(ref_m), " — loads under validate=true; bus/branch/gen/load/shunt identical per-unit")
else
    println("MISMATCH: ", basename(ref_m), " (", length(mismatches), ")")
    for m in first(mismatches, 40); println("  ", m); end
    exit(1)
end
