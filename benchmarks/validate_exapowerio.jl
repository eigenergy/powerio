# Validate powerio's parse against ExaPowerIO.jl, value for value, on a MATPOWER
# case. powerio's numbers come through its C ABI (see powerio_ffi.jl); ExaPowerIO
# is the reference Julia reader, parsed with its default filtered=true (which drops
# out-of-service rows and renumbers the survivors, so powerio's tables are filtered
# to the in-service rows to match). The comparison kernel — including the unit and
# encoding reconciliations (per-unit MW, b_fr+b_to split, radians vs degrees, tap
# 0→1) — is shared with the batched driver (oracle_compare.jl); this is the
# standalone one-case wrapper. Needs `cargo build --release -p powerio-capi`.
#
#   julia --project=benchmarks validate_exapowerio.jl tests/data/case14.m
using ExaPowerIO
include(joinpath(@__DIR__, "oracle_compare.jl"))
include(joinpath(@__DIR__, "powerio_ffi.jl"))

path = ARGS[1]
problems = compare_exapowerio(path)
name = basename(path)
if isempty(problems)
    println("MATCH: $name — bus/branch/gen counts and values identical")
else
    println("MISMATCH: $name ($(length(problems)))")
    for p in first(problems, 40)
        println("  ", p)
    end
    exit(1)
end
