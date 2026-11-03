# Binding/source-fidelity check against the external CSIRO09 native model.
# Run with --project pointing to the matching PowerIO.jl companion checkout.
using PowerIO, JSON3, SHA

length(ARGS) == 5 || error("usage: check_julia_bindings.jl SOURCE RECORDS CLI ACQUISITION_ROOT REPORT")
source, records, cli, root, report_path = abspath.(ARGS)
source_hash = "d2c41fae2f6fc3b9cdea75c1bcbc50f083b104440148a25274ee6ed7da57a14b"
record_hash = "fa661a3947bd171683d1b73a4e15c0d073c19c0df473f649fb32d4e8a2068592"
bytes2hex(sha256(read(source))) == source_hash || error("expected original CSIRO09 MDB")
bytes2hex(sha256(read(records))) == record_hash || error("expected verified CSIRO09 acquisition")
relative = relpath(records, dirname(source))
cases = []
powers = []
for hours in (0, 6)
    module_ = parse(source; format="sincal-multiconductor", acquisition_root=root,
        sincal_multiconductor=SincalReadOptions(variant=1, snapshot_hours=hours, acquired_tables=relative))
    GC.gc()
    module_ isa PioModule{MulticonductorNetwork} || error("wrong Julia network family")
    ir = serialize(module_).text
    expected = JSON3.read(ir)["value"]
    command = Cmd([cli, "serialize", source, "--from", "sincal-multiconductor",
        "--sincal-variant", "1", "--sincal-snapshot-hours", string(hours),
        "--sincal-acquired-tables", relative, "--acquisition-root", root])
    actual = JSON3.read(read(command, String))["value"]
    actual == expected || error("CLI and Julia typed values differ")
    bytes2hex(sha256(emit(module_, "sincal").artifacts[1].data)) == source_hash || error("original MDB echo differs")
    restored = deserialize(IOBuffer(ir))
    restored isa PioModule{MulticonductorNetwork} || error("IR restoration changed family")
    JSON3.read(serialize(restored).text)["value"] == expected || error("IR restoration changed typed value")
    push!(powers, [load.active_power_nominal_w for load in module_.value.loads])
    push!(cases, Dict("hours" => hours, "loads" => length(module_.value.loads),
        "typed_value_equal_to_cli" => true, "julia_original_mdb_echo" => true,
        "ir_typed_value_equal" => true))
end
powers[1] != powers[2] || error("snapshot selections did not change native load powers")
report = Dict(
    "scope" => "Julia/C binding selection and native source fidelity; not native SINCAL execution or independent solver evidence",
    "source_sha256" => source_hash, "record_sha256" => record_hash,
    "cli_sha256" => bytes2hex(sha256(read(cli))),
    "c_library_sha256" => bytes2hex(sha256(read(ENV["POWERIO_CAPI"]))),
    "julia_version" => string(VERSION), "cases" => cases, "snapshots_differ" => true,
    "passed" => true, "license" => "CC-BY-4.0",
    "attribution" => "Berry, Adam; Collins, Lyle; Oliver, Erin; Perfumo, Cristian (2015), Representative Australian Electricity Feeders with load and solar generation profiles, v1, CSIRO.")
open(report_path, "w") do io
    JSON3.pretty(io, report)
    println(io)
end
println("Julia/C: two CSIRO09 snapshots, typed CLI equality, original MDB echo and IR restoration passed")
