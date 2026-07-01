#!/usr/bin/env bash
set -u
set -o pipefail

OUT="${POWERIO_DATASET_BUNDLE_SMOKE_OUT:-target/dataset-bundle-smoke}"
LIMIT="${POWERIO_DATASET_BUNDLE_LIMIT:-25}"
DSS_LIMIT="${POWERIO_DATASET_BUNDLE_DSS_LIMIT:-40}"
BIN="${POWERIO_BIN:-target/debug/powerio}"

REPORT="$OUT/report.tsv"
COUNTS="$OUT/file-counts.txt"
LOG_DIR="$OUT/logs"
OUT_DIR="$OUT/out"

PASS=0
UNSUPPORTED=0
FAIL=0
TOTAL=0

if [ -z "${POWERIO_DATASET_BUNDLE:-}" ]; then
    echo "set POWERIO_DATASET_BUNDLE to the dataset bundle root" >&2
    exit 2
fi

ROOT="${POWERIO_DATASET_BUNDLE%/}"
if [ ! -d "$ROOT" ]; then
    echo "dataset root not found: $ROOT" >&2
    exit 2
fi

mkdir -p "$LOG_DIR" "$OUT_DIR"
: >"$REPORT"

echo "building powerio CLI" >&2
if ! cargo build -p powerio-cli --bin powerio; then
    echo "CLI build failed" >&2
    exit 1
fi

rel_path() {
    local path="$1"
    local prefix="$ROOT/"
    printf "%s" "${path#$prefix}"
}

slug() {
    local path="$1"
    local rel base hash
    rel="$(rel_path "$path")"
    base="$(basename "$rel" | sed 's/[^A-Za-z0-9_.-]/_/g')"
    hash="$(printf "%s" "$rel" | cksum | awk '{print $1}')"
    printf "%s_%s" "$hash" "$base"
}

classify_failure() {
    local log="$1"
    if grep -Eiq 'unsupported|not implemented|not supported|unknown format|failed to infer|cannot infer JSON format|pass --from to choose|no conversion path|cannot write|cannot read|read only|not a recognized|header magic mismatch|case has no buses|valid UTF-8|is display data, not a Network case' "$log"; then
        printf "UNSUPPORTED"
    else
        printf "FAIL"
    fi
}

run_capture() {
    local label="$1"
    local path="$2"
    shift 2
    local rel safe log status rc
    rel="$(rel_path "$path")"
    safe="$(printf "%s_%s" "$label" "$(slug "$path")" | sed 's/[^A-Za-z0-9_.-]/_/g')"
    log="$LOG_DIR/$safe.log"
    TOTAL=$((TOTAL + 1))
    "$@" >"$log" 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then
        PASS=$((PASS + 1))
        printf "PASS\t%s\t%s\t%s\n" "$label" "$rel" "$log" >>"$REPORT"
        return 0
    fi
    status="$(classify_failure "$log")"
    if [ "$status" = "UNSUPPORTED" ]; then
        UNSUPPORTED=$((UNSUPPORTED + 1))
    else
        FAIL=$((FAIL + 1))
    fi
    printf "%s\t%s\t%s\t%s\trc=%s\n" "$status" "$label" "$rel" "$log" "$rc" >>"$REPORT"
    return 1
}

run_verify_family() {
    local source="$1"
    local mfile="$2"
    local kind
    for kind in bprime bdoubleprime ybus_real ybus_imag; do
        run_capture "verify-$kind" "$source" "$BIN" verify "$mfile" --kind "$kind"
    done
}

run_transmission_case() {
    local path="$1"
    local from="$2"
    local tmp_m
    tmp_m="$OUT_DIR/$(slug "$path").m"

    run_capture "summary-$from" "$path" "$BIN" summary "$path" --from "$from"
    run_capture "package-$from" "$path" "$BIN" package "$path" --from "$from" -o "$OUT_DIR/$(slug "$path").pio.json"

    if [ "$from" = "matpower" ]; then
        run_verify_family "$path" "$path"
    elif run_capture "convert-$from-to-matpower" "$path" "$BIN" convert "$path" --from "$from" --to matpower -o "$tmp_m"; then
        run_verify_family "$path" "$tmp_m"
    fi
}

run_distribution_dss_case() {
    local path="$1"
    local safe
    safe="$(slug "$path")"
    run_capture "summary-dss" "$path" "$BIN" summary "$path" --from dss
    run_capture "package-dss" "$path" "$BIN" package "$path" --from dss -o "$OUT_DIR/$safe.pio.json"
    run_capture "convert-dss-to-bmopf" "$path" "$BIN" convert "$path" --from dss --to bmopf-json -o "$OUT_DIR/$safe.bmopf.json"
    run_capture "convert-dss-to-dss" "$path" "$BIN" convert "$path" --from dss --to dss -o "$OUT_DIR/$safe.canonical.dss"
}

run_json_case() {
    local path="$1"
    local safe
    safe="$(slug "$path")"
    run_capture "summary-json" "$path" "$BIN" summary "$path"
    run_capture "package-json" "$path" "$BIN" package "$path" -o "$OUT_DIR/$safe.pio.json"
}

list_matpower_cases() {
    find "$ROOT" -type f -iname '*.m' -print | sort | awk -v limit="$LIMIT" '
        function emit(p) { print p; n++; if (limit > 0 && n >= limit) exit }
        {
            p = tolower($0)
            b = p
            sub(/^.*\//, "", b)
            if (p ~ /\/pglib-opf\// ||
                b ~ /^case.*\.m$/ ||
                b ~ /^activsg.*\.m$/ ||
                b ~ /^texas.*\.m$/ ||
                b ~ /^hawaii.*\.m$/ ||
                b == "californiatestsystem.m" ||
                b == "rts_gmlc.m") {
                if (p !~ /contab|scenario|dynamics|startup|monitor|misc|run\.m|extract|interfaces/) {
                    emit($0)
                }
            }
        }'
}

list_ext_cases() {
    local ext="$1"
    find "$ROOT" -type f -iname "*.$ext" -print | sort | awk -v limit="$LIMIT" '
        function emit(p) { print p; n++; if (limit > 0 && n >= limit) exit }
        {
            p = tolower($0)
            if (p !~ /contingenc|dynamics|scenario|settings|diagram/) {
                emit($0)
            }
        }'
}

list_dss_roots() {
    find "$ROOT" -type f -iname '*.dss' -print | sort | awk -v limit="$DSS_LIMIT" '
        function emit(p) { print p; n++; if (limit > 0 && n >= limit) exit }
        {
            p = tolower($0)
            b = p
            sub(/^.*\//, "", b)
            if (b ~ /master.*\.dss$/ || b ~ /.*master.*\.dss$/) {
                emit($0)
            }
        }'
}

list_json_cases() {
    find "$ROOT" -type f -iname '*.json' -print | sort | awk -v limit="$LIMIT" '
        function emit(p) { print p; n++; if (limit > 0 && n >= limit) exit }
        {
            p = tolower($0)
            if (p !~ /schema|metadata|package-lock/) {
                emit($0)
            }
        }'
}

find "$ROOT" -type f \( \
    -iname '*.m' -o -iname '*.raw' -o -iname '*.epc' -o -iname '*.aux' -o \
    -iname '*.pwb' -o -iname '*.pwd' -o -iname '*.dss' -o -iname '*.json' \
    \) -print | awk '
        BEGIN { IGNORECASE = 1 }
        {
            n = split($0, a, ".")
            ext = tolower(a[n])
            count[ext]++
        }
        END {
            for (ext in count) {
                print count[ext], ext
            }
        }' | sort -nr >"$COUNTS"

while IFS= read -r path; do
    run_transmission_case "$path" matpower
done < <(list_matpower_cases)

while IFS= read -r path; do
    run_transmission_case "$path" psse
done < <(list_ext_cases raw)

while IFS= read -r path; do
    run_transmission_case "$path" pslf
done < <(list_ext_cases epc)

while IFS= read -r path; do
    run_transmission_case "$path" powerworld
done < <(list_ext_cases aux)

while IFS= read -r path; do
    run_transmission_case "$path" pwb
done < <(list_ext_cases pwb)

while IFS= read -r path; do
    # .pwd is a PowerWorld oneline display, not a Network case; there is no
    # `--from` case reader for it (see powerio/src/format/mod.rs display_file_guidance).
    # Confirm the CLI rejects it as a case rather than silently smoke testing
    # it through the wrong reader.
    run_capture "summary-pwd" "$path" "$BIN" summary "$path"
done < <(list_ext_cases pwd)

while IFS= read -r path; do
    run_distribution_dss_case "$path"
done < <(list_dss_roots)

while IFS= read -r path; do
    run_json_case "$path"
done < <(list_json_cases)

echo "dataset bundle smoke complete"
echo "commands: total=$TOTAL pass=$PASS unsupported=$UNSUPPORTED fail=$FAIL"
echo "report: $REPORT"
echo "file counts: $COUNTS"

if [ "$FAIL" -ne 0 ]; then
    echo "unexpected failures recorded in $REPORT" >&2
    exit 1
fi
