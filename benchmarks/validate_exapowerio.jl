# Validate caseio's parse against ExaPowerIO.jl, value for value, on a MATPOWER
# case. caseio's numbers come through its C ABI (see caseio_ffi.jl); ExaPowerIO is
# the reference Julia reader. Both read the same .m, so every electrical quantity
# must agree once the two encodings are reconciled:
#
#   - ExaPowerIO returns per-unit MW/MVAr (÷baseMVA); caseio returns raw MW. ×base.
#   - ExaPowerIO splits line charging into b_fr + b_to (each = b/2); caseio's b is
#     the total. Compare caseio.b == b_fr + b_to.
#   - ExaPowerIO stores shift / angle limits in radians; caseio in degrees.
#   - ExaPowerIO normalizes tap 0→1; caseio keeps the raw 0, so normalize both.
#   - ExaPowerIO rewrites bus types (PV/PQ/ref) from generator placement, so the
#     bus `type` is deliberately not compared.
#
# Parse ExaPowerIO with filtered=false so it keeps every bus/branch/gen (its
# default drops isolated buses and out-of-service elements, which would shift the
# counts away from caseio's lossless parse).
#
#   julia --project=benchmarks benchmarks/validate_exapowerio.jl tests/data/case14.m
#
# Exit 0 on a full match, 1 on any mismatch. Needs `cargo build --release -p caseio-capi`.

using ExaPowerIO
include(joinpath(@__DIR__, "caseio_ffi.jl"))

const ATOL = 1e-6
const RTOL = 1e-6

approx(a, b) = isapprox(float(a), float(b); atol = ATOL, rtol = RTOL)
eff_tap(t) = t == 0.0 ? 1.0 : float(t)

function main(path)
    name = basename(path)
    c = caseio_load(path)
    ed = ExaPowerIO.parse_matpower(path; filtered = false)
    base = ed.baseMVA

    problems = String[]
    push_count(label, a, b) = a == b || push!(problems, "$label count: caseio=$a exapowerio=$b")
    push_count("bus", c.n, length(ed.bus))
    push_count("branch", c.m, length(ed.branch))
    push_count("gen", c.ng, length(ed.gen))
    isempty(problems) || return report(name, problems)

    if abs(c.base_mva - base) > ATOL
        push!(problems, "baseMVA: caseio=$(c.base_mva) exapowerio=$base")
    end

    # Bus order should be file order on both sides; check the id vectors line up.
    exa_bus_id = [b.bus_i for b in ed.bus]
    if exa_bus_id != c.bus_ids
        push!(problems, "bus id order differs (caseio vs exapowerio)")
        return report(name, problems)
    end

    # Per-bus demand / shunt: ExaPowerIO per-unit → ×base to caseio's raw MW.
    for (k, b) in enumerate(ed.bus)
        approx(c.demand.pd[k], b.pd * base) || push!(problems, "bus[$(b.bus_i)].pd: caseio=$(c.demand.pd[k]) exa=$(b.pd*base)")
        approx(c.demand.qd[k], b.qd * base) || push!(problems, "bus[$(b.bus_i)].qd: caseio=$(c.demand.qd[k]) exa=$(b.qd*base)")
        approx(c.shunt.gs[k], b.gs * base)  || push!(problems, "bus[$(b.bus_i)].gs: caseio=$(c.shunt.gs[k]) exa=$(b.gs*base)")
        approx(c.shunt.bs[k], b.bs * base)  || push!(problems, "bus[$(b.bus_i)].bs: caseio=$(c.shunt.bs[k]) exa=$(b.bs*base)")
    end

    # Per-branch (file order). caseio from/to are dense 0-based; map to ids.
    for (k, br) in enumerate(ed.branch)
        cf_id = c.bus_ids[c.branch.from[k] + 1]
        ct_id = c.bus_ids[c.branch.to[k] + 1]
        ef_id = ed.bus[br.f_bus].bus_i
        et_id = ed.bus[br.t_bus].bus_i
        if (cf_id, ct_id) != (ef_id, et_id)
            push!(problems, "branch[$k] endpoints: caseio=($cf_id,$ct_id) exa=($ef_id,$et_id)")
            continue
        end
        approx(c.branch.r[k], br.br_r) || push!(problems, "branch[$k].r: caseio=$(c.branch.r[k]) exa=$(br.br_r)")
        approx(c.branch.x[k], br.br_x) || push!(problems, "branch[$k].x: caseio=$(c.branch.x[k]) exa=$(br.br_x)")
        approx(c.branch.b[k], br.b_fr + br.b_to) || push!(problems, "branch[$k].b: caseio=$(c.branch.b[k]) exa=$(br.b_fr + br.b_to)")
        approx(eff_tap(c.branch.tap[k]), eff_tap(br.tap)) || push!(problems, "branch[$k].tap: caseio=$(c.branch.tap[k]) exa=$(br.tap)")
        approx(c.branch.shift[k], rad2deg(br.shift)) || push!(problems, "branch[$k].shift: caseio=$(c.branch.shift[k]) exa(deg)=$(rad2deg(br.shift))")
    end

    # Per-gen (file order). caseio gen.bus dense 0-based; ExaPowerIO g.bus dense 1-based.
    for (k, g) in enumerate(ed.gen)
        cg_id = c.bus_ids[c.gen.bus[k] + 1]
        eg_id = ed.bus[g.bus].bus_i
        cg_id == eg_id || push!(problems, "gen[$k] bus: caseio=$cg_id exa=$eg_id")
        approx(c.gen.pg[k], g.pg * base)     || push!(problems, "gen[$k].pg: caseio=$(c.gen.pg[k]) exa=$(g.pg*base)")
        approx(c.gen.pmax[k], g.pmax * base) || push!(problems, "gen[$k].pmax: caseio=$(c.gen.pmax[k]) exa=$(g.pmax*base)")
        approx(c.gen.pmin[k], g.pmin * base) || push!(problems, "gen[$k].pmin: caseio=$(c.gen.pmin[k]) exa=$(g.pmin*base)")
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
