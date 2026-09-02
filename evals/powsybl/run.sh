#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 POWERIO_BINARY PYTHON POWSYBL_CORE_CHECKOUT" >&2
    exit 2
fi

powerio_binary=$1
python_binary=$2
powsybl_core=$3
repository_root=$(git rev-parse --show-toplevel)
output_dir=$(mktemp -d "${TMPDIR:-/tmp}/powerio-powsybl.XXXXXX")
trap 'rm -rf "$output_dir"' EXIT
powsybl_inputs="$output_dir/powsybl-inputs"

cgmes_2415_source="$powsybl_core/cgmes/cgmes-conformity/src/main/resources/conformity/cas-2/MicroGrid/Type2_T2/CGMES_v2.4.15_MicroGridTestConfiguration_T2_Assembled_Complete_v2"
cgmes_30_source="$powsybl_core/cgmes/cgmes-conformity/src/main/resources/conformity/cas-3-data-3.0.2/MicroGrid/BaseCase/MicroGrid-BaseCase-Merged"
remote_control_source="$powsybl_core/psse/psse-converter/src/test/resources/remoteControl.xiidm"
two_terminal_dc_source="$powsybl_core/psse/psse-converter/src/test/resources/twoTerminalDc.xiidm"
switched_shunt_source="$powsybl_core/psse/psse-converter/src/test/resources/SwitchedShunt.raw"
example_version_32_source="$powsybl_core/psse/psse-converter/src/test/resources/ExampleVersion32_exported.raw"
ieee_30_bus_32_source="$powsybl_core/psse/psse-converter/src/test/resources/IEEE_30_bus.raw"
two_substations_source="$powsybl_core/psse/psse-converter/src/test/resources/twoSubstations_rev35.rawx"
node_breaker_source="$powsybl_core/psse/psse-model-test/src/main/resources/five_bus_nodeBreaker_rev35.raw"
node_breaker_xiidm_source="$powsybl_core/psse/psse-converter/src/test/resources/five_bus_nodeBreaker_rev35.xiidm"
xiidm_serde_root="$powsybl_core/iidm/iidm-serde/src/test/resources"
cgmes_conformity_root="$powsybl_core/cgmes/cgmes-conformity/src/main/resources"
mini_grid_node_breaker_source="$cgmes_conformity_root/conformity/cas-1.1.3-data-4.0.3/MiniGrid/NodeBreaker/CGMES_v2.4.15_MiniGridTestConfiguration_BaseCase_Complete_v3"
mini_grid_node_breaker_bd_source="$cgmes_conformity_root/conformity/cas-1.1.3-data-4.0.3/MiniGrid/NodeBreaker/CGMES_v2.4.15_MiniGridTestConfiguration_Boundary_v3"
small_grid_node_breaker_source="$cgmes_conformity_root/conformity/cas-1.1.3-data-4.0.3/SmallGrid/NodeBreaker/CGMES_v2.4.15_SmallGridTestConfiguration_BaseCase_Complete_v3.0.0"
small_grid_node_breaker_bd_source="$cgmes_conformity_root/conformity/cas-1.1.3-data-4.0.3/SmallGrid/NodeBreaker/CGMES_v2.4.15_SmallGridTestConfiguration_Boundary_v3.0.0"
micro_grid_cgmes3_source="$cgmes_conformity_root/cgmes3-test-models/MicroGrid"

for source in \
    "$cgmes_2415_source" \
    "$cgmes_30_source" \
    "$remote_control_source" \
    "$two_terminal_dc_source" \
    "$switched_shunt_source" \
    "$example_version_32_source" \
    "$ieee_30_bus_32_source" \
    "$two_substations_source" \
    "$node_breaker_source" \
    "$node_breaker_xiidm_source" \
    "$mini_grid_node_breaker_source" \
    "$mini_grid_node_breaker_bd_source" \
    "$small_grid_node_breaker_source" \
    "$small_grid_node_breaker_bd_source" \
    "$micro_grid_cgmes3_source"; do
    if [[ ! -e "$source" ]]; then
        echo "missing PowSybl reference case: $source" >&2
        exit 1
    fi
done

for version in 12 13 14 15 16 17; do
    source="$xiidm_serde_root/V1_$version/threeWindingsTransformerToBeEstimated.xiidm"
    if [[ ! -f "$source" ]]; then
        echo "missing PowSybl XIIDM 1.$version reference case: $source" >&2
        exit 1
    fi
done

fresh_emit() {
    local source=$1
    local source_format=$2
    local target_format=$3
    local stem=$4
    local output=$5
    local ir="$output_dir/$stem.pio.json"

    "$powerio_binary" serialize "$source" --from "$source_format" -o "$ir"
    if ! "$powerio_binary" convert "$ir" --to "$target_format" -o "$output" \
        2> "$output_dir/$stem.emit.log"; then
        sed -n '1,$p' "$output_dir/$stem.emit.log" >&2
        return 1
    fi
    sed -n '1,$p' "$output_dir/$stem.emit.log" >&2
}

"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to xiidm -o "$output_dir/case9.xiidm"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to jiidm -o "$output_dir/case9.jiidm"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to cgmes -o "$output_dir/case9-cgmes"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse -o "$output_dir/case9-psse33.raw"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse34 -o "$output_dir/case9-psse34.raw"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse35 -o "$output_dir/case9-psse35.raw"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse-rawx -o "$output_dir/case9.rawx"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to ucte -o "$output_dir/case9.uct"
"$powerio_binary" convert "$repository_root/tests/data/case14.m" \
    --to ucte -o "$output_dir/case14.uct"

# Every UCTE-DEF fixture in the checkout: PowerIO reads it into IR and writes
# MATPOWER and fresh UCTE from the IR; the checker compares the IR with
# PyPowSybl's view and reloads the fresh UCTE. A fixture PowerIO refuses
# leaves a `.read.log` and no IR; the checker pins which ones those are.
ucte_dir="$output_dir/ucte"
mkdir -p "$ucte_dir"
find "$powsybl_core/ucte" -type f -name '*.uct' | sort | while read -r source; do
    relative=${source#"$powsybl_core/"}
    stem=${relative%.uct}
    stem=${stem//\//__}
    if "$powerio_binary" serialize "$source" --from ucte -o "$ucte_dir/$stem.pio.json" \
        2> "$ucte_dir/$stem.read.log"; then
        "$powerio_binary" convert "$ucte_dir/$stem.pio.json" --to matpower \
            -o "$ucte_dir/$stem.m" 2> "$ucte_dir/$stem.matpower.log"
        "$powerio_binary" convert "$ucte_dir/$stem.pio.json" --to ucte \
            -o "$ucte_dir/$stem.fresh.uct" 2> "$ucte_dir/$stem.emit.log"
    else
        rm -f "$ucte_dir/$stem.pio.json"
    fi
done

fresh_emit "$cgmes_2415_source" cgmes cgmes cgmes-2415 \
    "$output_dir/cgmes-2415"
fresh_emit "$cgmes_30_source" cgmes cgmes cgmes-30 \
    "$output_dir/cgmes-30"
fresh_emit "$remote_control_source" xiidm xiidm remote-control \
    "$output_dir/remote-control.xiidm"
fresh_emit "$remote_control_source" xiidm jiidm remote-control-jiidm \
    "$output_dir/remote-control.jiidm"
fresh_emit "$two_terminal_dc_source" xiidm xiidm two-terminal-dc \
    "$output_dir/two-terminal-dc.xiidm"
for version in 12 13 14 15 16 17; do
    fresh_emit \
        "$xiidm_serde_root/V1_$version/threeWindingsTransformerToBeEstimated.xiidm" \
        xiidm xiidm "xiidm-v1-$version" "$output_dir/xiidm-v1-$version.xiidm"
done
fresh_emit "$switched_shunt_source" psse psse35 switched-shunt \
    "$output_dir/switched-shunt.raw"
fresh_emit "$example_version_32_source" psse psse example-version-32 \
    "$output_dir/example-version-32.raw"
fresh_emit "$ieee_30_bus_32_source" psse psse ieee-30-bus-32 \
    "$output_dir/ieee-30-bus-32.raw"
fresh_emit "$two_substations_source" rawx psse-rawx two-substations \
    "$output_dir/two-substations.rawx"
fresh_emit "$node_breaker_source" psse psse35 five-bus-node-breaker \
    "$output_dir/five-bus-node-breaker.raw"
fresh_emit "$node_breaker_xiidm_source" xiidm xiidm five-bus-node-breaker-xiidm \
    "$output_dir/five-bus-node-breaker.xiidm"

"$python_binary" "$repository_root/evals/powsybl/export_inputs.py" \
    "$powsybl_core" "$powsybl_inputs"
for version in 12 13 14 15 16 17; do
    fresh_emit "$powsybl_inputs/powsybl-xiidm-1-$version.xiidm" \
        xiidm xiidm "powsybl-xiidm-1-$version" \
        "$output_dir/powsybl-xiidm-1-$version.xiidm"
done
for version in 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17; do
    fresh_emit "$powsybl_inputs/powsybl-eurostag-xiidm-1-$version.xiidm" \
        xiidm xiidm "powsybl-eurostag-xiidm-1-$version" \
        "$output_dir/powsybl-eurostag-xiidm-1-$version.xiidm"
    fresh_emit "$powsybl_inputs/powsybl-eurostag-jiidm-1-$version.jiidm" \
        jiidm jiidm "powsybl-eurostag-jiidm-1-$version" \
        "$output_dir/powsybl-eurostag-jiidm-1-$version.jiidm"
done
fresh_emit "$powsybl_inputs/powsybl-cgmes-2415.zip" \
    cgmes cgmes powsybl-cgmes-2415 "$output_dir/powsybl-cgmes-2415"
fresh_emit "$powsybl_inputs/powsybl-cgmes-30.zip" \
    cgmes cgmes powsybl-cgmes-30 "$output_dir/powsybl-cgmes-30"

# Node breaker sets are read twice: the official documents as shipped, and a
# copy without the TP and TP_BD documents whose buses PowerIO calculates.
node_breaker_dir="$output_dir/node-breaker"
stage_cgmes_documents() {
    local target=$1 mode=$2
    shift 2
    mkdir -p "$target"
    local source file
    for source in "$@"; do
        for file in "$source"/*.xml; do
            case "$mode:$(basename "$file")" in
                without-tp:*_TP_*|without-tp:*TP_BD*) ;;
                *) cp "$file" "$target/" ;;
            esac
        done
    done
}
run_logged() {
    local log=$1
    shift
    if ! "$@" 2> "$log"; then
        sed -n '1,$p' "$log" >&2
        return 1
    fi
}
calculated_topology() {
    local stem=$1
    shift
    stage_cgmes_documents "$node_breaker_dir/$stem-official" with-tp "$@"
    stage_cgmes_documents "$node_breaker_dir/$stem-without-tp" without-tp "$@"
    run_logged "$node_breaker_dir/$stem-official.log" "$powerio_binary" serialize \
        "$node_breaker_dir/$stem-official" --from cgmes \
        -o "$node_breaker_dir/$stem-official.pio.json"
    run_logged "$node_breaker_dir/$stem-calculated.log" "$powerio_binary" serialize \
        "$node_breaker_dir/$stem-without-tp" --from cgmes \
        -o "$node_breaker_dir/$stem-calculated.pio.json"
    run_logged "$node_breaker_dir/$stem-calculated-fresh.log" "$powerio_binary" convert \
        "$node_breaker_dir/$stem-calculated.pio.json" --to cgmes \
        -o "$node_breaker_dir/$stem-calculated-fresh"
}
calculated_topology mini-grid-node-breaker \
    "$mini_grid_node_breaker_source" "$mini_grid_node_breaker_bd_source"
calculated_topology small-grid-node-breaker \
    "$small_grid_node_breaker_source" "$small_grid_node_breaker_bd_source"
calculated_topology micro-grid-cgmes3 "$micro_grid_cgmes3_source"

"$python_binary" "$repository_root/evals/powsybl/check_outputs.py" \
    "$output_dir" "$powsybl_core"
