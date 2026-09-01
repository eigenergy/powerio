#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 POWERIO_BINARY PYTHON" >&2
    exit 2
fi

powerio_binary=$1
python_binary=$2
repository_root=$(git rev-parse --show-toplevel)
output_dir=$(mktemp -d "${TMPDIR:-/tmp}/powerio-powsybl.XXXXXX")
trap 'rm -rf "$output_dir"' EXIT

"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to xiidm -o "$output_dir/case9.xiidm"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to cgmes -o "$output_dir/case9-cgmes"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse35 -o "$output_dir/case9.raw"
"$powerio_binary" convert "$repository_root/tests/data/case9.m" \
    --to psse-rawx -o "$output_dir/case9.rawx"

"$python_binary" "$repository_root/evals/powsybl/check_outputs.py" "$output_dir"
