# Solve original and source-independent PowerIO exports with the same ACP model.
using PowerModelsACDC, Ipopt, JuMP, JSON, SHA, LinearAlgebra
include("common.jl")
using .AcDcValidation
PM.silence()
BLAS.set_num_threads(1)

const SETTINGS = Dict("conv_losses_mp" => true)
const OPTIONS = Dict{String,Any}("print_level" => 0, "sb" => "yes",
    "tol" => 1e-9, "max_iter" => 3000, "max_cpu_time" => 120.0,
    "bound_relax_factor" => 0.0, "linear_solver" => "mumps")
const FEASIBILITY_TOL = 1e-7
const OBJECTIVE_RTOL = 1e-8
const PF_ATOL = 1e-7
const PF_RTOL = 1e-6
const REFS = [PowerModelsACDC.add_ref_dcgrid!, PowerModelsACDC.ref_add_pst!,
    PowerModelsACDC.ref_add_sssc!, PowerModelsACDC.ref_add_flex_load!,
    PowerModelsACDC.ref_add_gendc!, PowerModelsACDC.ref_add_im!]

function variables_by_name(model)
    vars = all_variables(model)
    names = name.(vars)
    @assert all(!isempty, names) "cross-feasibility needs named variables"
    @assert length(Set(names)) == length(vars) "variable names must be unique"
    return Dict(zip(names, vars))
end

function residual(model, point=nothing)
    report = isnothing(point) ? primal_feasibility_report(model) : primal_feasibility_report(model, point)
    worst = maximum(values(report); init=0.0)
    @assert isfinite(worst) && worst <= FEASIBILITY_TOL "constraint/bound violation $worst exceeds $FEASIBILITY_TOL"
    return worst
end

function solve_case(data, kind)
    builder = kind == "pf" ? PowerModelsACDC.build_acdcpf : PowerModelsACDC.build_acdcopf
    pm = PM.instantiate_model(deepcopy(data), PM.ACPPowerModel, builder;
        ref_extensions=REFS, setting=deepcopy(SETTINGS))
    vars = variables_by_name(pm.model)
    starts = Dict(k => start_value(v) for (k, v) in vars)
    optimizer = optimizer_with_attributes(Ipopt.Optimizer, collect(OPTIONS)...)
    result = PM.optimize_model!(pm; optimizer)
    @assert result["termination_status"] in (JuMP.MOI.LOCALLY_SOLVED, JuMP.MOI.OPTIMAL) "$(kind): $(result["termination_status"])"
    @assert has_values(pm.model)
    values = Dict(k => value(v) for (k, v) in vars)
    @assert all(isfinite, Base.values(values))
    record = Dict("termination_status" => string(result["termination_status"]),
        "primal_status" => string(primal_status(pm.model)),
        "objective" => result["objective"], "max_primal_violation" => residual(pm.model),
        "starts" => starts, "solution" => result["solution"])
    return (; pm, vars, values, record)
end

# Evaluate each solved point in the other model, including all constraints and
# variable bounds. No inference from an equal objective alone is sufficient.
function cross_check(target, source)
    @assert Set(keys(target.vars)) == Set(keys(source.values))
    point = Dict(v => source.values[k] for (k, v) in target.vars)
    violation = residual(target.pm.model, point)
    objective = value(v -> point[v], objective_function(target.pm.model))
    @assert isapprox(objective, source.record["objective"]; atol=1e-7, rtol=OBJECTIVE_RTOL)
    return Dict("max_primal_violation" => violation, "objective" => objective)
end

function electrical_results(solution)
    result = Dict(table => deepcopy(solution[table])
        for table in ("bus", "gen", "branch", "busdc", "branchdc", "convdc"))
    # Derived from reference solution values, not a native PowerIO equation check.
    for c in values(result["convdc"])
        c["valve_loss_pu"] = c["pconv"] + c["pdc"]
        c["station_loss_pu"] = c["pgrid"] + c["pdc"]
    end
    for table in ("branch", "branchdc"), b in values(result[table])
        b["active_loss_pu"] = b["pf"] + b["pt"]
    end
    return result
end

function result_deltas(a, b, path=""; deltas=Dict{String,Float64}())
    @assert Set(keys(a)) == Set(keys(b)) "$path: different result fields"
    for k in keys(a)
        next = isempty(path) ? k : "$path/$k"
        if a[k] isa AbstractDict
            result_deltas(a[k], b[k], next; deltas)
        elseif a[k] isa Real && b[k] isa Real
            @assert isfinite(a[k]) && isfinite(b[k]) "$next: nonfinite result"
            deltas[next] = abs(a[k] - b[k])
        else
            @assert isequal(a[k], b[k]) "$next: different result values"
        end
    end
    return deltas
end

function compare_solve(original, fresh, kind)
    a, b = solve_case(original, kind), solve_case(fresh, kind)
    @assert a.record["termination_status"] == b.record["termination_status"]
    @assert a.record["primal_status"] == b.record["primal_status"]
    @assert a.record["starts"] == b.record["starts"] "different initialization"
    @assert isapprox(a.record["objective"], b.record["objective"]; atol=1e-7, rtol=OBJECTIVE_RTOL)
    left, right = electrical_results(a.record["solution"]), electrical_results(b.record["solution"])
    deltas = result_deltas(left, right)
    if kind == "pf"
        AcDcValidation.compare_values(left, right, "PF solution"; atol=PF_ATOL, rtol=PF_RTOL)
    end
    # OPF can have multiple optima: record state differences but qualify by
    # objective, feasibility and both cross-feasibility evaluations instead.
    record = Dict("source" => a.record, "fresh" => b.record,
        "source_point_in_fresh_model" => cross_check(b, a),
        "fresh_point_in_source_model" => cross_check(a, b),
        "absolute_result_differences" => deltas,
        "max_absolute_result_difference" => maximum(values(deltas); init=0.0))
    println("PASS: $kind objective=$(a.record["objective"]), max state difference=$(record["max_absolute_result_difference"])")
    return record
end

function main(args)
    @assert length(args) in (2, 3) "usage: solve.jl CASE OUTPUT_DIRECTORY [pf,opf|opf|pf]"
    source, directory = args[1:2]
    modes = length(args) == 3 ? split(args[3], ',') : ["pf", "opf"]
    @assert !isempty(modes) && all(k -> k in ("pf", "opf"), modes)
    raw, fresh = parse_case(source), parse_case(joinpath(directory, "fresh.m"))
    declared = compare_cases(raw, fresh)
    PowerModelsACDC.process_additional_data!(raw)
    PowerModelsACDC.process_additional_data!(fresh)
    effective = compare_cases(raw, fresh)
    report = Dict("julia" => string(VERSION), "powermodelsacdc" => string(pkgversion(PowerModelsACDC)),
        "powermodels" => string(pkgversion(PM)), "jump" => string(pkgversion(JuMP)),
        "ipopt" => string(pkgversion(Ipopt)), "manifest_sha256" => bytes2hex(sha256(read(joinpath(@__DIR__, "Manifest.toml")))),
        "source_sha256" => bytes2hex(sha256(read(source))),
        "fresh_sha256" => bytes2hex(sha256(read(joinpath(directory, "fresh.m")))),
        "model" => "ACPPowerModel", "settings" => SETTINGS, "optimizer_options" => OPTIONS,
        "feasibility_tolerance" => FEASIBILITY_TOL, "objective_rtol" => OBJECTIVE_RTOL,
        "pf_atol" => PF_ATOL, "pf_rtol" => PF_RTOL,
        "whole_case" => declared, "effective_comparison" => effective,
        "solves" => Dict(kind => compare_solve(raw, fresh, kind) for kind in modes))
    open(joinpath(directory, "solve.json"), "w") do io
        JSON.print(io, report, 2)
    end
end
abspath(PROGRAM_FILE) == (@__FILE__) && main(ARGS)
