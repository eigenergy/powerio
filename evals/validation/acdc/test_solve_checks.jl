# Verify that the numerical gate rejects bad points, not just solver failures.
using Test
include("solve.jl")
data = parse_case(normpath(joinpath(@__DIR__, "../../../tests/data/pglib-hvdc/case5_3_he.m")))
PowerModelsACDC.process_additional_data!(data)
solved = solve_case(data, "pf")
@testset "solver feasibility checks" begin
    @test residual(solved.pm.model) <= FEASIBILITY_TOL
    @test cross_check(solved, solved)["max_primal_violation"] <= FEASIBILITY_TOL
    bad = Dict(v => solved.values[k] for (k, v) in solved.vars)
    bad[solved.vars["0_vm[1]"]] += 0.1
    @test_throws AssertionError residual(solved.pm.model, bad)
    bad_source = merge(solved, (; record=merge(solved.record, Dict("objective" => 1.0))))
    @test_throws AssertionError cross_check(solved, bad_source)
end
