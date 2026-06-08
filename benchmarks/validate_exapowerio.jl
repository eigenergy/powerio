# Validate powerio's parse against ExaPowerIO.jl, value for value, on a MATPOWER
# case. powerio's numbers come through its C ABI (see powerio_ffi.jl); ExaPowerIO is
# the reference Julia reader, parsed with its default filtered=true. That default
# drops out-of-service branches/gens and isolated (type-4) buses and renumbers the
# survivors, so we filter powerio's tables to the in-service rows and compare those
# — the genuine test that powerio reproduces ExaPowerIO's filtered view. Encodings
# reconciled:
#
#   - ExaPowerIO returns per-unit MW/MVAr (÷baseMVA); powerio returns raw MW. ×base.
#   - ExaPowerIO splits line charging into b_fr + b_to (each = b/2); powerio's b is
#     the total. Compare powerio.b == b_fr + b_to.
#   - ExaPowerIO stores shift / angle limits in radians; powerio in degrees.
#   - ExaPowerIO normalizes tap 0→1; powerio keeps the raw 0, so normalize both.
#   - ExaPowerIO rewrites bus types (PV/PQ/ref) from generator placement, so the
#     bus `type` is deliberately not compared.
#
# Bus filtering on type 4 isn't exercised here (no fixture has isolated buses, and
# the C ABI doesn't surface the bus type); the in-service branch/gen filtering is.
#
#   julia --project=benchmarks benchmarks/validate_exapowerio.jl tests/data/case14.m
#
# Exit 0 on a full match, 1 on any mismatch. Needs `cargo build --release -p powerio-capi`.

using ExaPowerIO
include(joinpath(@__DIR__, "powerio_ffi.jl"))

const ATOL = 1e-6
const RTOL = 1e-6

approx(a, b) = isapprox(float(a), float(b); atol = ATOL, rtol = RTOL)
eff_tap(t) = t == 0.0 ? 1.0 : float(t)

function main(path)
    name = basename(path)
    c = powerio_load(path)
    ed = ExaPowerIO.parse_matpower(path)   # default filtered=true
    base = ed.baseMVA

    # powerio's in-service survivors, in file order, to line up with ExaPowerIO's
    # filtered, renumbered branch/gen lists.
    kb = [k for k in 1:c.m if c.branch.in_service[k] != 0]
    kg = [k for k in 1:c.ng if c.gen.in_service[k] != 0]

    problems = String[]
    push_count(label, a, b) = a == b || push!(problems, "$label count: powerio=$a exapowerio=$b")
    push_count("bus", c.n, length(ed.bus))
    push_count("branch (in service)", length(kb), length(ed.branch))
    push_count("gen (in service)", length(kg), length(ed.gen))
    isempty(problems) || return report(name, problems)

    if abs(c.base_mva - base) > ATOL
        push!(problems, "baseMVA: powerio=$(c.base_mva) exapowerio=$base")
    end

    # Buses aren't dropped here (no type-4 fixtures), so they stay in file order.
    exa_bus_id = [b.bus_i for b in ed.bus]
    if exa_bus_id != c.bus_ids
        push!(problems, "bus id order differs (powerio vs exapowerio)")
        return report(name, problems)
    end

    # Per-bus demand / shunt: ExaPowerIO per-unit → ×base to powerio's raw MW.
    for (k, b) in enumerate(ed.bus)
        approx(c.demand.pd[k], b.pd * base) || push!(problems, "bus[$(b.bus_i)].pd: powerio=$(c.demand.pd[k]) exa=$(b.pd*base)")
        approx(c.demand.qd[k], b.qd * base) || push!(problems, "bus[$(b.bus_i)].qd: powerio=$(c.demand.qd[k]) exa=$(b.qd*base)")
        approx(c.shunt.gs[k], b.gs * base)  || push!(problems, "bus[$(b.bus_i)].gs: powerio=$(c.shunt.gs[k]) exa=$(b.gs*base)")
        approx(c.shunt.bs[k], b.bs * base)  || push!(problems, "bus[$(b.bus_i)].bs: powerio=$(c.shunt.bs[k]) exa=$(b.bs*base)")
    end

    # Per in-service branch. powerio from/to are 1-based bus ids (same id space as
    # pio_bus_ids), so compare them to ExaPowerIO's bus ids directly.
    for (j, k) in enumerate(kb)
        br = ed.branch[j]
        cf_id = c.branch.from[k]
        ct_id = c.branch.to[k]
        ef_id = ed.bus[br.f_bus].bus_i
        et_id = ed.bus[br.t_bus].bus_i
        if (cf_id, ct_id) != (ef_id, et_id)
            push!(problems, "branch[$k] endpoints: powerio=($cf_id,$ct_id) exa=($ef_id,$et_id)")
            continue
        end
        approx(c.branch.r[k], br.br_r) || push!(problems, "branch[$k].r: powerio=$(c.branch.r[k]) exa=$(br.br_r)")
        approx(c.branch.x[k], br.br_x) || push!(problems, "branch[$k].x: powerio=$(c.branch.x[k]) exa=$(br.br_x)")
        approx(c.branch.b[k], br.b_fr + br.b_to) || push!(problems, "branch[$k].b: powerio=$(c.branch.b[k]) exa=$(br.b_fr + br.b_to)")
        approx(eff_tap(c.branch.tap[k]), eff_tap(br.tap)) || push!(problems, "branch[$k].tap: powerio=$(c.branch.tap[k]) exa=$(br.tap)")
        approx(c.branch.shift[k], rad2deg(br.shift)) || push!(problems, "branch[$k].shift: powerio=$(c.branch.shift[k]) exa(deg)=$(rad2deg(br.shift))")
    end

    # Per in-service gen. powerio gen.bus is a 1-based bus id; ExaPowerIO g.bus is a
    # dense 1-based index into ed.bus.
    for (j, k) in enumerate(kg)
        g = ed.gen[j]
        cg_id = c.gen.bus[k]
        eg_id = ed.bus[g.bus].bus_i
        cg_id == eg_id || push!(problems, "gen[$k] bus: powerio=$cg_id exa=$eg_id")
        approx(c.gen.pg[k], g.pg * base)     || push!(problems, "gen[$k].pg: powerio=$(c.gen.pg[k]) exa=$(g.pg*base)")
        approx(c.gen.pmax[k], g.pmax * base) || push!(problems, "gen[$k].pmax: powerio=$(c.gen.pmax[k]) exa=$(g.pmax*base)")
        approx(c.gen.pmin[k], g.pmin * base) || push!(problems, "gen[$k].pmin: powerio=$(c.gen.pmin[k]) exa=$(g.pmin*base)")
    end

    return report(name, problems)
end

function report(name, problems)
    if isempty(problems)
        println("MATCH: $name — bus/branch/gen counts and values identical")
        return 0
    else
        println("MISMATCH: $name ($(length(problems)))")
        for p in first(problems, 40)
            println("  ", p)
        end
        return 1
    end
end

exit(main(ARGS[1]))
