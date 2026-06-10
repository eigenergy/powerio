# PMD oracle for powerio-dist.
#
# Usage:
#   julia pmdtool.jl dss2json input.dss output.json   # ENGINEERING model JSON
#   julia pmdtool.jl check input.json                 # parse_file must accept it
#
# Set PIO_PMD_PATH to develop a local PowerModelsDistribution clone instead of
# the registered release. First run resolves the project; later runs reuse it.

import Pkg
Pkg.activate(@__DIR__; io = devnull)

loaded = try
    @eval using PowerModelsDistribution, JSON
    true
catch
    false
end
if !loaded
    pmd_path = get(ENV, "PIO_PMD_PATH", "")
    if isempty(pmd_path)
        Pkg.add("PowerModelsDistribution")
    else
        Pkg.develop(path = pmd_path)
    end
    Pkg.add("JSON")
    Pkg.instantiate()
    @eval using PowerModelsDistribution, JSON
end

function main(argv)
    if length(argv) == 3 && argv[1] == "dss2json"
        eng = parse_file(argv[2]; kron_reduce = false)
        open(argv[3], "w") do io
            print_file(io, eng)
        end
        println("wrote $(argv[3])")
        return 0
    elseif length(argv) == 2 && argv[1] == "check"
        data = parse_file(argv[2])
        println("parsed: data_model=$(data["data_model"]) components=$(length(keys(data)))")
        return 0
    end
    println(stderr, "usage: julia pmdtool.jl dss2json in.dss out.json | check in.json")
    return 2
end

exit(main(ARGS))
