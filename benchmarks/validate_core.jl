# Generic PowerModels electrical-core comparator: one reference, one or more test
# files. bus/branch/gen/load counts and the demand/generation/shunt totals after
# make_per_unit!. The comparison kernel is shared with the batched driver
# (oracle_compare.jl); this is the standalone wrapper.
#
#   julia --project=benchmarks validate_core.jl <ref> <test1> [test2 ...]
#
# Prints "OK <test>" or "FAIL <test>: ..." per file; exits nonzero if any fail.
using PowerModels
PowerModels.silence()
include(joinpath(@__DIR__, "oracle_compare.jl"))

fails = 0
ref = ARGS[1]
for test_path in ARGS[2:end]
    problems = compare_core(ref, test_path)
    if isempty(problems)
        println("OK $test_path")
    else
        println("FAIL $test_path: ", join(problems, "; "))
        global fails += 1
    end
end
exit(fails == 0 ? 0 : 1)
