# Whole-case and native-declaration checks; solve checks are in solve.jl.
using PowerModelsACDC, JSON, SHA
include("common.jl")
using .AcDcValidation
PM.silence()
source, directory = ARGS
raw = parse_case(source)
fresh = parse_case(joinpath(directory, "fresh.m"))
whole_case = compare_cases(raw, fresh)
clean(row) = Dict(k => v for (k,v) in row if k ∉ ("source_id", "index"))
for table in ["busdc", "branchdc", "convdc"]
    @assert Set(keys(raw[table])) == Set(keys(fresh[table])) table
    for (id,row) in raw[table]
        @assert clean(row) == clean(fresh[table][id]) (table,id,clean(row),clean(fresh[table][id]))
    end
end
@assert raw["dcpol"] == fresh["dcpol"]
document = JSON.parsefile(joinpath(directory,"source.pio.json"))
e = document["value"]["data"]["ac_dc_equivalent"]
@assert e["pole_count"] == raw["dcpol"]
@assert e["base_mva"] == raw["baseMVA"]
dcids = Dict(b["component"]["local_id"] => b["external_id"] for b in e["buses"])
number(component) = dcids[component["local_id"]]
for (i,b) in enumerate(e["buses"])
    expected = Dict("busdc_i"=>b["external_id"],
        "grid" => b["source_grid"],
        "Pdc" => b["withdrawal_mw"],
        "Vdc" => b["initial_voltage_pu"],
        "basekVdc" => b["base_kv"],
        "Vdcmax" => b["voltage_limits_pu"]["max"],
        "Vdcmin" => b["voltage_limits_pu"]["min"],
        "Cdc" => b["capacitance_pu"])
    b["area"] !== nothing && (expected["area"] = b["area"])
    @assert expected == clean(raw["busdc"][string(i)])
end
for (i,b) in enumerate(e["branches"])
    expected = Dict("fbusdc"=>number(b["from"]),
        "tbusdc" => number(b["to"]),
        "r" => b["resistance_pu"],
        "l" => b["inductance_pu_per_second"],
        "c" => b["capacitance_pu_seconds"],
        "rateA" => b["ratings_mw"][1],
        "rateB" => b["ratings_mw"][2],
        "rateC" => b["ratings_mw"][3],
        "status" => Int(b["in_service"]))
    @assert expected == clean(raw["branchdc"][string(i)])
end
for (i,c) in enumerate(e["stations"])
    s=c["settings"]; tf=c["transformer"]; re=c["reactor"]; loss=c["losses"]
    dc = Dict("active_power_at_pcc"=>1,
        "dc_voltage" => 2,
        "dc_power_droop" => 3)
    ac = Dict("reactive_power_at_pcc"=>1,
        "voltage_at_pcc" => 2)
    expected = Dict("busdc_i"=>number(c["dc_bus"]),
        "busac_i" => c["ac_bus"],
        "type_dc" => dc[c["dc_control"]],
        "type_ac" => ac[c["ac_control"]],
        "P_g" => -s["pcc_withdrawal_mw"],
        "Q_g" => -s["pcc_withdrawal_mvar"],
        "islcc" => Int(c["technology"]=="lcc"),
        "Vtar" => s["declared_ac_voltage_pu"],
        "rtf" => tf["resistance_pu"],
        "xtf" => tf["reactance_pu"],
        "transformer" => Int(tf["enabled"]),
        "tm" => c["transformer_tap_ratio"],
        "bf" => c["filter_susceptance_pu"],
        "filter" => Int(c["filter_enabled"]),
        "rc" => re["resistance_pu"],
        "xc" => re["reactance_pu"],
        "reactor" => Int(re["enabled"]),
        "basekVac" => c["ac_base_kv"],
        "Vmmax" => c["valve_voltage_pu"]["max"],
        "Vmmin" => c["valve_voltage_pu"]["min"],
        "Imax" => c["valve_current_limit_pu"],
        "status" => Int(c["in_service"]),
        "LossA" => loss["constant_mw"],
        "LossB" => loss["linear_kv"],
        "LossCrec" => loss["rectifier_quadratic_ohm"],
        "LossCinv" => loss["inverter_quadratic_ohm"],
        "droop" => s["droop"],
        "Pdcset" => s["dc_withdrawal_target_mw"],
        "Vdcset" => s["dc_voltage_target_pu"],
        "dVdcset" => s["dc_voltage_deadband_pu"],
        "Pacmax" => c["valve_active_mw"]["max"],
        "Pacmin" => c["valve_active_mw"]["min"],
        "Qacmax" => c["valve_reactive_mvar"]["max"],
        "Qacmin" => c["valve_reactive_mvar"]["min"])
    @assert expected == clean(raw["convdc"][string(i)]) (i,expected,clean(raw["convdc"][string(i)]))
end
# Separate snapshots keep source limits distinct from preprocessing adjustments.
effective = deepcopy(raw)
PowerModelsACDC.process_additional_data!(effective)
effective_fresh = deepcopy(fresh)
PowerModelsACDC.process_additional_data!(effective_fresh)
effective_comparison = compare_cases(effective, effective_fresh)
snapshot = Dict("julia"=>string(VERSION),
        "source_sha256" => bytes2hex(sha256(read(source))),
        "fresh_sha256" => bytes2hex(sha256(read(joinpath(directory,"fresh.m")))),
        "alias_policy" => "rename PGLib aliases in parsed dictionaries before PowerModelsACDC preprocessing",
        "whole_case" => whole_case,
        "effective_comparison" => effective_comparison,
        "source" => raw,
        "effective" => effective)
open(joinpath(directory,"reference.json"),"w") do io; JSON.print(io,snapshot); end
println("PASS: whole-case inputs, native AC/DC declarations and effective preprocessing match")
