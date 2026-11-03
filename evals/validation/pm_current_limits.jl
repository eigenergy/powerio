# Print PowerModels' derived branch current limits for a MATPOWER case, one
# `index c_rating_a` line per branch in branch order. The AC OTS
# `DeriveFromRateA` policy reproduces `calc_current_limits!`; the reference
# values in powerio-prob/tests/ac_ots.rs come from this script.
#
#   julia --project=evals/validation pm_current_limits.jl <case.m>
using PowerModels
PowerModels.silence()
data = PowerModels.parse_file(ARGS[1])
PowerModels.calc_current_limits!(data)
for index in sort(parse.(Int, collect(keys(data["branch"]))))
    branch = data["branch"][string(index)]
    println(index, " ", repr(branch["c_rating_a"]))
end
