# Batched cross-tool validation driver: load PowerModels and ExaPowerIO ONCE and
# run every oracle leg for every fixture in a single process, instead of spawning
# a fresh `julia` (and re-`using PowerModels`) per case — the bulk of the
# validation CI wall time. The per-leg comparison kernels live in oracle_compare.jl;
# the standalone validate_*.jl wrappers reuse the same kernels for one-off checks.
#
#   julia --project=benchmarks validate_oracles.jl export  <tmp> <case.m>...
#   julia --project=benchmarks validate_oracles.jl compare <tmp> --m <m>... --raw <r>... --egret <e>...
#
# `export` writes <tmp>/<base>.pmref.json (PowerModels' own per_unit JSON) for the
# PMread leg — it must run before powerio re-emits it. `compare` runs PMjson /
# PMread / PSSE / Exa over the MATPOWER cases, PSSE-read over the .raw cases, and
# EGRET-read over the EGRET cases, reading powerio's conversions from <tmp> (named
# by convention, see run_validation.sh). It appends one `<case>\t<leg>\t<mark>`
# line per leg to <tmp>/results.tsv and prints detail; exits nonzero on any FAIL.
using PowerModels
using ExaPowerIO
PowerModels.silence()
include(joinpath(@__DIR__, "oracle_compare.jl"))
include(joinpath(@__DIR__, "powerio_ffi.jl"))

stem(path) = first(splitext(basename(path)))

# A leg that reads a powerio conversion fails cleanly if that conversion is
# missing or empty (the Python convert phase reports the underlying error).
present(path) = (isfile(path) && filesize(path) > 0) ? nothing : "missing or empty: $(basename(path))"

# Run one leg: require `files` to exist, run `cmp` (caught so one leg can't abort
# the driver), record the mark to results.tsv, print detail. `cmp` returns the
# problems vector, or a (problems, note) tuple for an informational annotation.
# Returns true on pass.
function leg!(io, case, name, files, cmp)
    miss = filter(!isnothing, present.(files))
    problems, note = if !isempty(miss)
        (String.(miss), "")
    else
        try
            r = cmp()
            r isa Tuple ? r : (r, "")
        catch e
            (["exception: $e"], "")
        end
    end
    ok = isempty(problems)
    println(io, "$case\t$name\t$(ok ? "ok" : "FAIL")")
    if ok
        println("MATCH: $case $name$note")
    else
        println("MISMATCH: $case $name$note ($(length(problems)))")
        for p in first(problems, 40)
            println("  ", p)
        end
    end
    return ok
end

function do_export(tmp, mcases)
    for m in mcases
        PowerModels.export_file(joinpath(tmp, stem(m) * ".pmref.json"), PowerModels.parse_file(m))
    end
end

function do_compare(tmp, groups)
    fails = 0
    open(joinpath(tmp, "results.tsv"), "a") do io
        for m in groups["m"]
            b = stem(m)
            pmjson = joinpath(tmp, "$b.pmjson.json")
            pmref, reemit = joinpath(tmp, "$b.pmref.json"), joinpath(tmp, "$b.reemit.json")
            psse = joinpath(tmp, "$b.psse.raw")

            leg!(io, b, "PMjson", [pmjson], () -> compare_powermodels(m, pmjson)) || (fails += 1)
            leg!(io, b, "PMread", [pmref, reemit], () -> compare_powermodels(pmref, reemit)) || (fails += 1)
            leg!(io, b, "PSSE", [psse], () -> compare_psse(m, psse)) || (fails += 1)
            leg!(io, b, "Exa", String[], () -> compare_exapowerio(m)) || (fails += 1)
        end

        for r in groups["raw"]
            b = stem(r)
            json = joinpath(tmp, "psse_$b.json")
            leg!(io, "psse/$b", "PSSE-read", [json], () -> compare_psse(r, json)) || (fails += 1)
        end

        for e in groups["egret"]
            b = stem(e)
            json = joinpath(tmp, "egret_$b.json")
            mref = joinpath(@__DIR__, "..", "tests", "data", "$b.m")
            leg!(io, "egret/$b", "EGRET-read", [json], () -> compare_core(mref, json)) || (fails += 1)
        end
    end
    return fails
end

function parse_groups(args)
    groups = Dict("m" => String[], "raw" => String[], "egret" => String[])
    cur = nothing
    for a in args
        if startswith(a, "--")
            cur = a[3:end]
            haskey(groups, cur) || error("unknown group flag --$cur")
        else
            cur === nothing && error("argument before any --group flag: $a")
            push!(groups[cur], a)
        end
    end
    return groups
end

function main(args)
    isempty(args) && error("usage: validate_oracles.jl export|compare <tmp> ...")
    mode, tmp, rest = args[1], args[2], args[3:end]
    if mode == "export"
        do_export(tmp, rest)
        return 0
    elseif mode == "compare"
        return do_compare(tmp, parse_groups(rest)) == 0 ? 0 : 1
    else
        error("unknown mode $mode (expected export or compare)")
    end
end

exit(main(ARGS))
