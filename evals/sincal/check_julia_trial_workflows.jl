using PowerIO, Test, JSON3
# Use the matching local PowerIO.jl branch and a freshly built C ABI library.
# Sources stay external; no result table or native model is redistributed.
length(ARGS) == 4 || error("expected CSIRO19.mdb records19.json CSIRO12.mdb records12.json")
source19, records19, source12, records12 = ARGS
@testset "Native SINCAL option integration" begin
    native = read(source19)
    records = read(records19)
    for hours in [0, 12, 24]
        module_ = parse(native; format="sincal-balanced",
            sincal_balanced=SincalBalancedReadOptions(variant=1, snapshot_hours=hours, acquired_tables="records.json"),
            named_buffers=Dict("records.json" => records))
        @test module_ isa PioModule{BalancedNetwork}
        @test emit(module_, "sincal").artifacts[1].data == native
        @test deserialize(IOBuffer(serialize(module_).text)) isa PioModule{BalancedNetwork}
        @test to_ac_pf_instance(module_) isa PioModule{AcPfInstance}
    end
    native = read(source12)
    records = read(records12)
    for compatibility in [false, true]
        selection = SincalReadOptions(variant=1, snapshot_hours=12, acquired_tables="records.json",
            assume_inactive_source_controls=compatibility)
        if !compatibility
            @test_throws PowerIOError parse(native; format="sincal-multiconductor",
                sincal_multiconductor=selection, named_buffers=Dict("records.json" => records))
        else
            module_ = parse(native; format="sincal-multiconductor",
                sincal_multiconductor=selection, named_buffers=Dict("records.json" => records))
            GC.gc()
            @test module_ isa PioModule{MulticonductorNetwork}
            @test length(module_.value.loads) == 26
            @test emit(module_, "sincal").artifacts[1].data == native
            @test to_mc_ac_pf_instance(module_) isa PioModule{McAcPfInstance}
            @test deserialize(IOBuffer(serialize(module_).text)) isa PioModule{MulticonductorNetwork}
        end
    end
end
