# Negative controls: these gates must detect electrical changes, not just run.
using Test
include("common.jl")
using .AcDcValidation
PM.silence()
const CASE = normpath(joinpath(@__DIR__, "../../../tests/data/pglib-hvdc/case5_3_he.m"))
source = parse_case(CASE)
@testset "whole-case input comparison" begin
    @test compare_cases(source, deepcopy(source)) isa Dict
    for (table, field) in [("bus", "vm"), ("bus", "bus_type"),
            ("gen", "pg"), ("gen", "pmax"), ("gen", "qmin"), ("gen", "gen_status"),
            ("branch", "tap"), ("branch", "shift"), ("branch", "rate_a"),
            ("branch", "br_status"), ("branch", "index"), ("convdc", "P_g"),
            ("convdc", "Q_g"), ("convdc", "LossA"), ("busdc", "basekVdc")]
        bad = deepcopy(source)
        bad[table]["1"][field] += 1
        @test_throws AssertionError compare_cases(source, bad)
    end
    bad = deepcopy(source)
    bad["gen"]["1"]["cost"][1] += 1
    @test_throws AssertionError compare_cases(source, bad)
    bad = deepcopy(source)
    delete!(bad["branch"], "1")
    @test_throws AssertionError compare_cases(source, bad)
    bad = deepcopy(source)
    bad["branch"]["1"]["unrecognized_electrical_limit"] = 3
    @test_throws AssertionError compare_cases(source, bad)
    bad = deepcopy(source)
    bad["bus"]["1"]["vm"] = NaN
    @test_throws AssertionError compare_cases(source, bad)
    bad = deepcopy(source)
    bad["dcpol"] = 1
    @test_throws AssertionError compare_cases(source, bad)
    annotations = deepcopy(source)
    annotations["name"] = "fresh"
    annotations["source_version"] = "different-file-layout"
    annotations["gen"]["1"]["mu_pmax"] = 1.0
    report = compare_cases(source, annotations)
    @test report["fresh_exclusions"]["gen/1/mu_pmax"] == 1.0
end
