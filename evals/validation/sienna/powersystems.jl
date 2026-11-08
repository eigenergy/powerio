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
    ids = first(PowerNetworkMatrices.get_axes(y))
    entries = [[ids[i], ids[j], real(matrix[i,j]), imag(matrix[i,j])] for i in axes(matrix,1) for j in axes(matrix,2) if matrix[i,j] != 0]
    open(path, "w") do io
        JSON3.write(io, entries)
    end
end
for name in ["case9", "case14", "case30", "case57", "case118"]
    reference = System(joinpath(cases, name * ".m"); runchecks=false)
    to_json(reference, joinpath(inputs, name * ".json"); force=true)
    write_ybus(reference, joinpath(inputs, name * ".ybus.json"))
    # Native 4.x input is tested by Rust; fresh native output targets format 5.
    if pkgversion(PowerSystems).major >= 5
        restored = System(joinpath(exports, "powersystems-5", name * ".json"))
        write_ybus(restored, joinpath(inputs, name * ".export-ybus.json"))
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
