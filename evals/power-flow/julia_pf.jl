using JSON
using SparseArrays

const scheme = ARGS[1]
if scheme == "powermodels"
    using PowerModels
    PowerModels.silence()
elseif scheme == "powersystems"
    using PowerSystems, PowerFlows
else
    using PowerModelsDistribution
end

function sienna(path)
    sys = System(path)
    result = solve_power_flow(ACPowerFlow(), sys)
    voltage = Dict(string(r.bus_number) => [r.Vm, rad2deg(r.θ)] for r in eachrow(result["bus_results"]))
    powers = [[string(r.bus_from), string(r.bus_to), r.P_from_to, r.Q_from_to, r.P_to_from, r.Q_to_from]
              for r in eachrow(result["flow_results"])]
    set_units_base_system!(sys, "SYSTEM_BASE")
    ybus = PowerFlows.PNM.Ybus(sys)
    rows, cols, entries = findnz(PowerFlows.PNM.get_data(ybus))
    y = Dict("basis" => "network-pu", "base_mva" => get_base_power(sys),
             "labels" => string.(first(axes(ybus))),
             "entries" => [[r-1, c-1, real(v), imag(v)] for (r, c, v) in zip(rows, cols, entries)])
    return Dict("solver" => "PowerFlows $(Base.pkgversion(PowerFlows)), PowerSystems $(Base.pkgversion(PowerSystems))",
                "units" => "pu", "voltage" => voltage, "branch_power" => powers, "admittance" => y)
end

function balanced(path)
    data = PowerModels.parse_file(path)
    result = PowerModels.compute_ac_pf(data)
    result["termination_status"] === true || error("PowerModels did not converge")
    voltage = Dict(key => [row["vm"], rad2deg(row["va"])] for (key, row) in result["solution"]["bus"])
    PowerModels.update_data!(data, result["solution"])
    flows = PowerModels.calc_branch_flow_ac(data)["branch"]
    keys = ("pf", "qf", "pt", "qt")
    branch_power = [[string(data["branch"][key]["f_bus"]), string(data["branch"][key]["t_bus"]),
                     (data["baseMVA"] .* [row[field] for field in keys])...]
                    for (key, row) in flows if all(field -> isfinite(row[field]), keys)]
    answer = Dict("solver" => "PowerModels $(Base.pkgversion(PowerModels))", "units" => "pu",
                  "voltage" => voltage, "branch_power" => branch_power)
    if isempty(data["dcline"]) && isempty(data["switch"])
        y = PowerModels.calc_admittance_matrix(data)
        rows, cols, entries = findnz(y.matrix)
        answer["admittance"] = Dict("basis" => "network-pu", "base_mva" => data["baseMVA"],
                                     "labels" => string.(y.idx_to_bus),
                                     "entries" => [[r-1, c-1, real(v), imag(v)] for (r, c, v) in zip(rows, cols, entries)])
    end
    return answer
end

function distribution(path)
    PMD = PowerModelsDistribution
    data = PMD.parse_file(isdir(path) ? joinpath(path, "case.dss") : path)
    engineering = PMD.iseng(data)
    result = PMD.compute_mc_pf(data; make_si=engineering, stat_tol=1e-10)
    string(result["termination_status"]) == "PF_CONVERGED" || error("PowerModelsDistribution did not converge: $(result["termination_status"])")
    voltage = Dict{String,Vector{Float64}}()
    for (key, row) in result["solution"]["bus"]
        bus = data["bus"][key]
        terminals = bus["terminals"]
        ground = engineering ? Set(bus["grounded"]) : Set(terminals[findall(isone, bus["grounded"])])
        if engineering && length(row["vm"]) != length(terminals)
            terminals = [t for t in terminals if !(t in ground)]
        end
        length(terminals) == length(row["vm"]) == length(row["va"]) ||
            error("Voltage results do not identify all terminals at bus $key")
        for (terminal, vm, va) in zip(terminals, row["vm"], row["va"])
            terminal in ground && continue
            scale = data["settings"]["voltage_scale_factor"] * (engineering ? 1 : bus["vbase"])
            label = engineering ? key : get(bus, "name", key)
            voltage["$(lowercase(label)).$terminal"] = [vm * scale, engineering ? va : rad2deg(va)]
        end
    end
    answer = Dict{String,Any}("solver" => "PowerModelsDistribution $(Base.pkgversion(PMD))",
                             "units" => "V", "voltage" => voltage,
                             "one_island" => engineering ? length(data["voltage_source"]) == 1 :
                                 count(bus -> bus["bus_type"] == 3, values(data["bus"])) == 1)
    try
        math_data = engineering ? PMD.transform_data_model(data) : deepcopy(data)
        PMD.add_start_voltage!(math_data; coordinates=:rectangular, epsilon=0, explicit_neutral=false)
        pfd = PMD.PowerFlowData(math_data, PMD._bts_to_start_voltage(math_data), false)
        labels = String[]
        bases = Float64[]
        for node in pfd.indexed_nodes
            node isa Tuple || error("Matrix contains internal voltage constraints without physical node labels")
            bus, terminal = node
            record = math_data["bus"][string(bus)]
            push!(labels, "$(lowercase(get(record, "name", string(bus)))).$terminal")
            push!(bases, record["vbase"] * math_data["settings"]["voltage_scale_factor"])
        end
        sbase = math_data["settings"]["sbase"] * math_data["settings"]["power_scale_factor"]
        rows, cols, entries = findnz(pfd.Y0)
        values_si = [y * sbase / (bases[r] * bases[c]) for (r, c, y) in zip(rows, cols, entries)]
        answer["admittance"] = Dict("basis" => "PowerModelsDistribution network Y0, in siemens",
                                    "base_mva" => nothing, "labels" => labels,
                                    "method" => "PowerModelsDistribution PowerFlowData.Y0",
                                    "entries" => [[r-1, c-1, real(v), imag(v)] for (r, c, v) in zip(rows, cols, values_si)])
    catch e
        answer["admittance_detail"] = sprint(showerror, e)
    end
    return answer
end

results = Dict{String,Any}()
for (key, path) in JSON.parsefile(ARGS[2])
    results[key] = try
        solve = scheme == "powermodels" ? balanced : scheme == "powersystems" ? sienna : distribution
        answer = solve(path)
        isempty(answer["voltage"]) && error("The solver returned no bus voltages")
        answer["status"] = "solved"
        answer
    catch e
        Dict("status" => "solve_failed", "detail" => sprint(showerror, e))
    end
end
open(ARGS[3], "w") do io
    JSON.print(io, results)
end
