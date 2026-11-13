# Invoke with a pinned PowerSystems project, an exports directory, and MATPOWER data.
using PowerSystems
using Test
using PowerNetworkMatrices
using JSON3

exports, cases, inputs = ARGS
mkpath(inputs)
function write_ybus(system, path)
    set_units_base_system!(system, "SYSTEM_BASE")
    y = Ybus(system)
    matrix = PowerNetworkMatrices.get_data(y)
    ids = first(axes(y))
    entries = [(ids[i], ids[j], real(matrix[i,j]), imag(matrix[i,j])) for i in axes(matrix,1) for j in axes(matrix,2) if matrix[i,j] != 0]
    open(path, "w") do io
        JSON3.write(io, (entries=entries, epsilon=Float64(eps(typeof(real(zero(eltype(matrix))))))))
    end
end
function write_costs(system, path)
    entries = Tuple{Int,Float64,Float64}[]
    for bus in get_components(ACBus, system), mw in [0.0, 25.0, 100.0]
        total = 0.0
        for generator in get_components(ThermalStandard, system)
            get_number(get_bus(generator)) == get_number(bus) || continue
            cost = get_operation_cost(generator)
            variable = get_variable(cost)
            units = string(get_power_units(variable))
            scale = units == "DEVICE_BASE" ? get_base_power(generator) : units == "SYSTEM_BASE" ? get_base_power(system) : 1.0
            x = mw / scale
            curve = get_value_curve(variable)
            quadratic = applicable(get_quadratic_term, curve) ? get_quadratic_term(curve) : 0.0
            vom = get_vom_cost(variable)
            total += quadratic*x*x + get_proportional_term(curve)*x + get_constant_term(curve) + get_proportional_term(vom)*x + get_constant_term(vom) + get_fixed(cost)
        end
        push!(entries, (get_number(bus), mw, total))
    end
    open(path, "w") do io
        JSON3.write(io, entries)
    end
end
for name in ["case9", "case14", "case30", "case57", "case118"]
    reference = System(joinpath(cases, name * ".m"); runchecks=false)
    to_json(reference, joinpath(inputs, name * ".json"); force=true)
    write_ybus(reference, joinpath(inputs, name * ".ybus.json"))
    write_costs(reference, joinpath(inputs, name * ".costs.json"))
    # Native 4.x input is tested by Rust; fresh native output targets format 5.
    if pkgversion(PowerSystems).major >= 5
        restored = System(joinpath(exports, "powersystems-5", name * ".json"))
        write_ybus(restored, joinpath(inputs, name * ".export-ybus.json"))
        write_costs(restored, joinpath(inputs, name * ".export-costs.json"))
        @test length(get_components(ACBus, restored)) == length(get_components(ACBus, reference))
        @test length(get_components(Generator, restored)) == length(get_components(Generator, reference))
        for getter in [get_active_power, get_reactive_power]
            @test sum(getter(g) for g in get_components(Generator, restored)) ≈ sum(getter(g) for g in get_components(Generator, reference))
            @test sum(getter(g) for g in get_components(PowerLoad, restored)) ≈ sum(getter(g) for g in get_components(PowerLoad, reference))
        end
        signature(b) = (get_number(get_from(get_arc(b))), get_number(get_to(get_arc(b))), get_r(b), get_x(b))
        a = sort(signature.(collect(get_components(ACBranch, restored))))
        b = sort(signature.(collect(get_components(ACBranch, reference))))
        @test length(a) == length(b)
        @test all(all(isapprox.(x, y; atol=1e-10, rtol=1e-10)) for (x, y) in zip(a,b))
    end
end
if pkgversion(PowerSystems).major >= 5
    for path in filter(p -> startswith(basename(p), "three_winding"), readdir(joinpath(exports,"powersystems-5"); join=true))
        restored = System(path)
        @test length(get_components(ThreeWindingTransformer, restored)) == 1
    end
end

if pkgversion(PowerSystems).major >= 5
    shunts = System(joinpath(exports, "powersystems-5", "switched_shunt.json"))
    shunt = only(get_components(SwitchedAdmittance, shunts))
    set_units_base_system!(shunts, "SYSTEM_BASE")
    @test imag(get_Y(shunt)) ≈ 0.125
    @test get_number_of_steps(shunt) == [3, 2]
    if isdefined(PowerSystems, :get_control_mode) && applicable(PowerSystems.get_control_mode, shunt)
        @test string(get_control_mode(shunt)) == "DISCRETE_VOLTAGE"
        @test get_regulated_bus_number(shunt) == 5
    else
        @test get_ext(shunt)["org.eigenergy.powerio"]["switched_control_fields"]["regulated_bus_number"] == 5
    end
    to_json(shunts, joinpath(inputs, "switched_shunt.json"); force=true)
end

if pkgversion(PowerSystems).major >= 5
    storage_system = System(joinpath(exports, "powersystems-5", "storage_costs.json"))
    set_units_base_system!(storage_system, "NATURAL_UNITS")
    storage = only(get_components(EnergyReservoirStorage, storage_system))
    @test get_storage_capacity(storage) ≈ 300.0
    @test get_active_power(storage) ≈ -30.0
    @test get_initial_storage_capacity_level(storage) ≈ 0.25
end
