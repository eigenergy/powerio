# Print the per-unit electrical core of each PowerModels-readable file, one line
# per file: "<path>\t<n_bus> <n_branch> <n_gen> <n_load> <n_shunt> <Σpd> <Σqd>
# <Σpg> <Σgs> <Σbs>" (totals after make_per_unit!, so the units match whatever
# oracle read the file). Used by validate_matrix.py to compare a conversion's core
# against its source's core across MATPOWER / PowerModels JSON / PSS-E outputs in
# a single Julia process.
#
#   julia --project=benchmarks core_json.jl <file1> [file2 ...]
using PowerModels
PowerModels.silence()

for path in ARGS
    local d
    try
        d = PowerModels.parse_file(path)
        PowerModels.make_per_unit!(d)
    catch e
        println(path, "\tERR ", e)
        continue
    end
    tot(et, f) = round(sum(e[f] for e in values(get(d, et, Dict())); init = 0.0), digits = 6)
    cnt(et) = length(get(d, et, Dict()))
    println(
        path, "\t",
        cnt("bus"), " ", cnt("branch"), " ", cnt("gen"), " ", cnt("load"), " ", cnt("shunt"),
        " ", tot("load", "pd"), " ", tot("load", "qd"), " ", tot("gen", "pg"),
        " ", tot("shunt", "gs"), " ", tot("shunt", "bs"),
    )
end
