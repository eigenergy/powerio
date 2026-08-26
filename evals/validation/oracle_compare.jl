# Shared PowerModels / ExaPowerIO comparison kernels, factored out of the
# individual validate_*.jl scripts. The batched driver (validate_oracles.jl)
# includes this once so it can load PowerModels and ExaPowerIO a single time and
# run every case in one process, instead of paying `using PowerModels` per case
# (the bulk of the validation CI time). The standalone validate_*.jl wrappers
# include it too, so one case can still be checked on its own.
#
# Every kernel returns a Vector{String} of problems (empty == match). The caller
# provides `using PowerModels` (and `using ExaPowerIO` + powerio_ffi.jl for the
# ExaPowerIO kernel) before including this file.

const _SKIP_FIELDS = ("source_id", "index")

_approx(a, b) =
    (a isa Number && b isa Number) ? isapprox(float(a), float(b); atol = 1e-9, rtol = 1e-7) : a == b

# JSON has no ±Inf/NaN, so an unbounded reference value (e.g. a pegase gen
# qmax=Inf) arrives as `nothing` on the JSON side. Restore it from `ref` so the
# two compare equal (both mean unbounded) and make_per_unit! doesn't trip.
function _restore_inf!(data, ref)
    for et in ("bus", "branch", "gen", "load", "shunt")
        haskey(ref, et) || continue
        for (k, dv) in get(data, et, Dict())
            rv = get(ref[et], k, nothing)
            rv === nothing && continue
            for (f, val) in dv
                if val === nothing && haskey(rv, f) && rv[f] isa Number && !isfinite(rv[f])
                    dv[f] = rv[f]
                end
            end
        end
    end
end

_total(d, et, f) = round(sum(e[f] for e in values(get(d, et, Dict())); init = 0.0), digits = 4)
_count(d, et) = length(get(d, et, Dict()))

# powerio's PowerModels JSON vs PowerModels' own parse of the .m, value for value
# over bus/branch/gen/load/shunt. Two checks: (1) the powerio JSON must load under
# PowerModels' default validate=true (the interop property that motivates emitting
# per_unit=true); (2) parse both with validate=false and per-unitize explicitly so
# correct_network_data! can't rewrite both sides into agreement and hide a bug.
function compare_powermodels(ref_m::AbstractString, our_json::AbstractString)
    try
        PowerModels.parse_file(our_json)
    catch e
        return ["powerio JSON rejected by PowerModels validate=true: $e"]
    end

    ref = PowerModels.parse_file(ref_m; validate = false)
    ours = PowerModels.parse_file(our_json; validate = false)
    PowerModels.make_per_unit!(ref)
    PowerModels.make_per_unit!(ours)
    _restore_inf!(ours, ref)

    problems = String[]
    for et in ("bus", "branch", "gen", "load", "shunt")
        r = get(ref, et, Dict())
        o = get(ours, et, Dict())
        if length(r) != length(o)
            push!(problems, "$et: count $(length(r)) (ref) vs $(length(o)) (ours)")
            continue
        end
        for (k, rv) in r
            haskey(o, k) || (push!(problems, "$et[$k]: missing in ours"); continue)
            ov = o[k]
            for (f, rfv) in rv
                f in _SKIP_FIELDS && continue
                if !haskey(ov, f)
                    push!(problems, "$et[$k].$f: missing in ours (ref=$rfv)")
                elseif !_approx(rfv, ov[f])
                    push!(problems, "$et[$k].$f: ref=$rfv ours=$(ov[f])")
                end
            end
            for f in keys(ov)
                f in _SKIP_FIELDS && continue
                haskey(rv, f) || push!(problems, "$et[$k].$f: extra in ours (=$(ov[f]))")
            end
        end
    end
    return problems
end

function _rich_has_field(raw, table, fields)
    for (_, item) in get(raw, table, Dict())
        any(haskey(item, f) for f in fields) && return true
    end
    return false
end

function _rich_cmp!(problems, raw, parsed, table, fields; label = table)
    r = get(raw, table, Dict())
    p = get(parsed, table, Dict())
    for (k, rv) in r
        haskey(p, k) || (push!(problems, "$label[$k]: missing after PowerModels parse"); continue)
        pv = p[k]
        for f in fields
            haskey(rv, f) || continue
            if !haskey(pv, f)
                push!(problems, "$label[$k].$f: missing after PowerModels parse")
            elseif !_approx(rv[f], pv[f])
                push!(problems, "$label[$k].$f: raw=$(rv[f]) parsed=$(pv[f])")
            end
        end
    end
end

function _load_bus_hist(loads)
    hist = Dict{Any, Int}()
    for (_, load) in loads
        bus = load["load_bus"]
        hist[bus] = get(hist, bus, 0) + 1
    end
    return hist
end

# Validate the richer PowerModels JSON tables directly against PowerModels'
# own parsed data dict. This is intentionally separate from compare_powermodels:
# the legacy comparator keeps the electrical core matrix readable, while this
# one proves fields beyond MATPOWER's row shape are not rejected or collapsed by
# the PowerModels network data model.
function compare_powermodels_rich(json_path::AbstractString)
    raw = JSON.parsefile(json_path)
    parsed = try
        PowerModels.parse_file(json_path; validate = false)
    catch e
        return ["PowerModels rejected rich JSON: $e"]
    end

    problems = String[]
    rich_seen = 0

    raw_loads = get(raw, "load", Dict())
    parsed_loads = get(parsed, "load", Dict())
    if length(raw_loads) != length(parsed_loads)
        push!(problems, "load count: raw=$(length(raw_loads)) parsed=$(length(parsed_loads))")
    end
    if _load_bus_hist(raw_loads) != _load_bus_hist(parsed_loads)
        push!(problems, "load_bus multiplicity changed")
    elseif any(v > 1 for v in values(_load_bus_hist(raw_loads)))
        rich_seen += 1
    end

    branch_fields = ("g_fr", "b_fr", "g_to", "b_to", "c_rating_a", "c_rating_b", "c_rating_c", "pf", "qf", "pt", "qt")
    _rich_has_field(raw, "branch", branch_fields) && (rich_seen += 1)
    _rich_cmp!(problems, raw, parsed, "branch", branch_fields)

    switch_fields = ("f_bus", "t_bus", "state", "thermal_rating", "current_rating", "pf", "qf", "pt", "qt")
    !isempty(get(raw, "switch", Dict())) && (rich_seen += 1)
    _rich_cmp!(problems, raw, parsed, "switch", switch_fields)

    storage_fields = ("storage_bus", "thermal_rating", "current_rating", "energy_rating", "charge_rating", "discharge_rating")
    _rich_has_field(raw, "storage", storage_fields) && (rich_seen += 1)
    _rich_cmp!(problems, raw, parsed, "storage", storage_fields)

    dcline_fields = ("model", "ncost", "cost")
    _rich_has_field(raw, "dcline", dcline_fields) && (rich_seen += 1)
    _rich_cmp!(problems, raw, parsed, "dcline", dcline_fields)

    rich_seen > 0 || push!(problems, "no rich fields found in $(basename(json_path))")
    return problems
end

# Generic PowerModels electrical-core comparator: bus/branch/gen/load counts and
# the demand/generation totals are strict, shunt count is informational (powerio
# models fixed shunts but not PSS/E switched shunts). Returns (problems, note).
function compare_psse(ref_path::AbstractString, test_path::AbstractString)
    ref = PowerModels.parse_file(ref_path)
    test = PowerModels.parse_file(test_path)
    _restore_inf!(test, ref)
    PowerModels.make_per_unit!(ref)
    PowerModels.make_per_unit!(test)

    problems = String[]
    for et in ("bus", "branch", "gen", "load")
        _count(ref, et) == _count(test, et) ||
            push!(problems, "$et count: ref=$(_count(ref, et)) test=$(_count(test, et))")
    end
    for (et, f) in (("load", "pd"), ("load", "qd"), ("gen", "pg"))
        r, t = _total(ref, et, f), _total(test, et, f)
        isapprox(r, t; atol = 1e-6, rtol = 1e-6) || push!(problems, "Σ$et.$f: ref=$r test=$t")
    end
    # When the shunt counts agree, the admittances must match too.
    sref, stest = _count(ref, "shunt"), _count(test, "shunt")
    if sref == stest
        for f in ("gs", "bs")
            r, t = _total(ref, "shunt", f), _total(test, "shunt", f)
            isapprox(r, t; atol = 1e-6, rtol = 1e-6) || push!(problems, "Σshunt.$f: ref=$r test=$t")
        end
    end
    note = sref == stest ? "" : "  (shunt: ref=$sref test=$stest — switched shunts not modeled)"
    return problems, note
end

# Core comparator against a per-unitized reference, totals including shunt gs/bs
# checked unconditionally so a dropped or mis-scaled element shows up regardless
# of how each oracle buckets elements per bus.
function compare_core(ref_path::AbstractString, test_path::AbstractString)
    ref = PowerModels.parse_file(ref_path)
    PowerModels.make_per_unit!(ref)
    local test
    try
        test = PowerModels.parse_file(test_path)
    catch e
        return ["parse error $e"]
    end
    _restore_inf!(test, ref)
    PowerModels.make_per_unit!(test)

    problems = String[]
    for et in ("bus", "branch", "gen", "load")
        _count(ref, et) == _count(test, et) ||
            push!(problems, "$et count ref=$(_count(ref, et)) test=$(_count(test, et))")
    end
    for (et, f) in (("load", "pd"), ("load", "qd"), ("gen", "pg"), ("shunt", "gs"), ("shunt", "bs"))
        r, t = _total(ref, et, f), _total(test, et, f)
        isapprox(r, t; atol = 1e-6, rtol = 1e-6) || push!(problems, "Σ$et.$f ref=$r test=$t")
    end
    return problems
end

_eff_tap(t) = t == 0.0 ? 1.0 : float(t)
_xa(a, b) = isapprox(float(a), float(b); atol = 1e-6, rtol = 1e-6)

# powerio (through its C ABI, see powerio_ffi.jl) vs ExaPowerIO.jl, value for
# value on the in-service rows ExaPowerIO keeps (default filtered=true). The
# caller must `using ExaPowerIO` and include powerio_ffi.jl first.
function compare_exapowerio(path::AbstractString)
    c = powerio_load(path)
    ed = ExaPowerIO.parse_matpower(path)   # default filtered=true
    base = ed.baseMVA

    # powerio's in-service survivors, in file order, to line up with ExaPowerIO's
    # filtered, renumbered branch/gen lists.
    kb = [k for k in 1:c.m if c.branch.in_service[k] != 0]
    kg = [k for k in 1:c.ng if c.gen.in_service[k] != 0]

    problems = String[]
    pc(label, a, b) = a == b || push!(problems, "$label count: powerio=$a exapowerio=$b")
    pc("bus", c.n, length(ed.bus))
    pc("branch (in service)", length(kb), length(ed.branch))
    pc("gen (in service)", length(kg), length(ed.gen))
    isempty(problems) || return problems

    abs(c.base_mva - base) > 1e-6 && push!(problems, "baseMVA: powerio=$(c.base_mva) exapowerio=$base")

    exa_bus_id = [b.bus_i for b in ed.bus]
    if exa_bus_id != c.bus_ids
        push!(problems, "bus id order differs (powerio vs exapowerio)")
        return problems
    end

    for (k, b) in enumerate(ed.bus)
        _xa(c.demand.pd[k], b.pd * base) || push!(problems, "bus[$(b.bus_i)].pd: powerio=$(c.demand.pd[k]) exa=$(b.pd*base)")
        _xa(c.demand.qd[k], b.qd * base) || push!(problems, "bus[$(b.bus_i)].qd: powerio=$(c.demand.qd[k]) exa=$(b.qd*base)")
        _xa(c.shunt.gs[k], b.gs * base) || push!(problems, "bus[$(b.bus_i)].gs: powerio=$(c.shunt.gs[k]) exa=$(b.gs*base)")
        _xa(c.shunt.bs[k], b.bs * base) || push!(problems, "bus[$(b.bus_i)].bs: powerio=$(c.shunt.bs[k]) exa=$(b.bs*base)")
    end

    for (j, k) in enumerate(kb)
        br = ed.branch[j]
        cf_id, ct_id = c.branch.from[k], c.branch.to[k]
        ef_id, et_id = ed.bus[br.f_bus].bus_i, ed.bus[br.t_bus].bus_i
        if (cf_id, ct_id) != (ef_id, et_id)
            push!(problems, "branch[$k] endpoints: powerio=($cf_id,$ct_id) exa=($ef_id,$et_id)")
            continue
        end
        _xa(c.branch.r[k], br.br_r) || push!(problems, "branch[$k].r: powerio=$(c.branch.r[k]) exa=$(br.br_r)")
        _xa(c.branch.x[k], br.br_x) || push!(problems, "branch[$k].x: powerio=$(c.branch.x[k]) exa=$(br.br_x)")
        _xa(c.branch.b[k], br.b_fr + br.b_to) || push!(problems, "branch[$k].b: powerio=$(c.branch.b[k]) exa=$(br.b_fr + br.b_to)")
        _xa(_eff_tap(c.branch.tap[k]), _eff_tap(br.tap)) || push!(problems, "branch[$k].tap: powerio=$(c.branch.tap[k]) exa=$(br.tap)")
        _xa(c.branch.shift[k], rad2deg(br.shift)) || push!(problems, "branch[$k].shift: powerio=$(c.branch.shift[k]) exa(deg)=$(rad2deg(br.shift))")
    end

    for (j, k) in enumerate(kg)
        g = ed.gen[j]
        cg_id, eg_id = c.gen.bus[k], ed.bus[g.bus].bus_i
        cg_id == eg_id || push!(problems, "gen[$k] bus: powerio=$cg_id exa=$eg_id")
        _xa(c.gen.pg[k], g.pg * base) || push!(problems, "gen[$k].pg: powerio=$(c.gen.pg[k]) exa=$(g.pg*base)")
        _xa(c.gen.pmax[k], g.pmax * base) || push!(problems, "gen[$k].pmax: powerio=$(c.gen.pmax[k]) exa=$(g.pmax*base)")
        _xa(c.gen.pmin[k], g.pmin * base) || push!(problems, "gen[$k].pmin: powerio=$(c.gen.pmin[k]) exa=$(g.pmin*base)")
    end

    return problems
end
