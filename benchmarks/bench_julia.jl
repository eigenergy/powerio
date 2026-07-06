# Cross tool parse and Ybus benchmark in one Julia process. The headline PowerIO
# rows use the public PowerIO.jl API; the raw C ABI rows stay as lower bound
# diagnostics for separating parser core time from Julia wrapper time.
#
#   cargo build --release -p powerio-capi --features arrow,matrix  # once
#   julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'   # once
#   julia --project=benchmarks benchmarks/bench_julia.jl
#
# Fetch the large cases first: `bash benchmarks/fetch_cases.sh`.
# Numbers are per machine; record them in RESULTS.md rather than hardcoding.
#
# Note on the bus count: ExaPowerIO defaults to `filtered=true`, which drops
# isolated (type-4) buses, so on cases that have them its count is below powerio's
# full, lossless count. That is a parse policy difference; the value level
# cross checks live in validate_exapowerio.jl / validate_pandapower.py.

const POWERIO_JL_CHECKOUT = normpath(joinpath(@__DIR__, "..", "..", "PowerIO.jl"))
isdir(POWERIO_JL_CHECKOUT) && pushfirst!(LOAD_PATH, POWERIO_JL_CHECKOUT)

using ExaPowerIO, PowerModels, BenchmarkTools, SparseArrays, Logging, PowerIO
using JSON, Dates, Statistics
PowerModels.silence()
include(joinpath(@__DIR__, "powerio_ffi.jl"))
PowerIO.set_library!(get(ENV, "POWERIO_CAPI", LIBPOWERIO))

# (name, path, run_powermodels_parse?, run_powermodels_ybus?)
# A false PowerModels flag means this harness records n/a for that column; it is
# not a wall time claim. Parse and Ybus are separate because some cases parse but
# PowerModels cannot build Ybus for them.
const CASES = [
    ("case2869pegase", "tests/data/case2869pegase.m", true, true),
    ("case_ACTIVSg2000", "tests/data/large/case_ACTIVSg2000.m", true, true),
    ("case9241pegase", "tests/data/large/case9241pegase.m", true, true),
    ("case13659pegase", "tests/data/large/case13659pegase.m", true, true),
    ("case_ACTIVSg10k", "tests/data/large/case_ACTIVSg10k.m", true, true),
    ("case_ACTIVSg25k", "tests/data/large/case_ACTIVSg25k.m", true, true),
    ("case_ACTIVSg70k", "tests/data/large/case_ACTIVSg70k.m", true, true),
    ("case_SyntheticUSA", "tests/data/large/case_SyntheticUSA.m", true, false),
    ("case99k", "tests/data/large/case99k.m", false, false),
    ("case193k", "tests/data/large/case193k.m", false, false),
]

function trial_stats(b; digits = 2)
    times_ms = Float64.(b.times) ./ 1e6
    return (
        ms = round(median(times_ms), digits = digits),
        std_ms = round(length(times_ms) > 1 ? std(times_ms) : 0.0, digits = digits),
        n = length(times_ms),
    )
end

show_stat(s) = "$(s.ms) +/- $(s.std_ms) ms"

function git_commit()
    try
        return chomp(read(`git -C $(normpath(joinpath(@__DIR__, ".."))) rev-parse HEAD`, String))
    catch
        return nothing
    end
end

function benchmark_metadata()
    return (
        benchmark_time_utc = Dates.format(Dates.now(Dates.UTC), dateformat"yyyy-mm-ddTHH:MM:SS.sss") * "Z",
        git_commit = git_commit(),
        command = join(vcat(["julia", "--project=benchmarks", "benchmarks/bench_julia.jl"], ARGS), " "),
    )
end

# `--json`: also write benchmarks/results/speed_julia.json for render_tables.py.
const JSON_OUT = "--json" in ARGS
jrows = NamedTuple[]
matrix_jrows = NamedTuple[]

function exapowerio_parse_matpower(path)
    with_logger(NullLogger()) do
        ExaPowerIO.parse_matpower(path)
    end
end

function powermodels_parse_ybus(path)
    data = PowerModels.parse_file(path)
    PowerModels.make_per_unit!(data)
    PowerModels.calc_admittance_matrix(data).matrix
end

function exapowerio_parse_ybus(path)
    data = exapowerio_parse_matpower(path)
    n = length(data.bus)
    rows = Int[]
    cols = Int[]
    vals = ComplexF64[]
    sizehint!(rows, 4 * length(data.branch) + n)
    sizehint!(cols, 4 * length(data.branch) + n)
    sizehint!(vals, 4 * length(data.branch) + n)

    for bus in data.bus
        y = bus.gs + im * bus.bs
        if y != 0
            push!(rows, Int(bus.i)); push!(cols, Int(bus.i)); push!(vals, y)
        end
    end

    for br in data.branch
        br.status == 0 && continue
        f = Int(br.f_bus)
        t = Int(br.t_bus)
        push!(rows, f); push!(cols, f); push!(vals, br.c1 + im * br.c2)
        push!(rows, f); push!(cols, t); push!(vals, br.c3 + im * br.c4)
        push!(rows, t); push!(cols, f); push!(vals, br.c5 + im * br.c6)
        push!(rows, t); push!(cols, t); push!(vals, br.c7 + im * br.c8)
    end
    return sparse(rows, cols, vals, n, n)
end

function write_speed_julia(path, rows, matrix_rows)
    mkpath(dirname(path))
    open(path, "w") do io
        JSON.print(io, (metadata = benchmark_metadata(), rows = rows, matrix_rows = matrix_rows), 2)
        println(io)
    end
    println("wrote $path ($(length(rows)) rows)")
end

function free_network!(net)
    net === nothing && return
    h = getfield(net, :handle)
    h === nothing || h.ptr == C_NULL || finalize(h)
    return
end

function powerio_jl_materialize_data(path)
    net = PowerIO.parse_file(path)
    try
        return net.data
    finally
        free_network!(net)
    end
end

println(rpad("case", 20), rpad("PowerIO.jl", 24), rpad("ExaPowerIO", 24),
        rpad("PowerModels", 24), rpad("Rust C ABI", 24), rpad("net.data", 24),
        "buses (PowerIO / ExaPowerIO)")
for (name, f, run_pm_parse, _run_pm_ybus) in CASES
    if !isfile(f)
        @warn "bench_julia: fixture missing, dropping it from the run" case = name path = f
        continue
    end

    h = pio_parse_file(f); nbuses = pio_n_buses(h); nbranch = pio_n_branches(h); pio_free(h)
    samples = nbuses > 30_000 ? 5 : 30

    netref = Ref{Any}(nothing)
    bj = @benchmark $netref[] = PowerIO.parse_file($f) teardown = (free_network!($netref[]); $netref[] = nothing) samples = samples evals = 1
    pj = trial_stats(bj)

    data = nothing
    data_display = "skip"
    if nbuses <= 15_000
        bd = @benchmark powerio_jl_materialize_data($f) samples = samples evals = 1
        data = trial_stats(bd)
        data_display = show_stat(data)
    end

    href = Ref{Ptr{Cvoid}}(C_NULL)
    bc = @benchmark $href[] = pio_parse_file($f) teardown = (pio_free($href[])) samples = samples evals = 1
    rust_c = trial_stats(bc)

    ed = exapowerio_parse_matpower(f)
    be = @benchmark exapowerio_parse_matpower($f) samples = samples evals = 1
    e = trial_stats(be)

    pm = nothing
    p = "skip"
    if run_pm_parse
        try
            bp = @benchmark PowerModels.parse_file($f) samples = 5 evals = 1 seconds = 60
            pm = trial_stats(bp; digits = 1)
            p = show_stat(pm)
        catch e
            @warn "PowerModels parse benchmark failed" case = name error = sprint(showerror, e)
        end
    end

    count = nbuses == length(ed.bus) ? string(nbuses) : "$nbuses / $(length(ed.bus))"
    println(rpad(name, 20), rpad(show_stat(pj), 24), rpad(show_stat(e), 24),
            rpad(p, 24), rpad(show_stat(rust_c), 24), rpad(data_display, 24), count)
    push!(jrows, (case = name, buses = nbuses, branches = nbranch,
                  powerio_jl_ms = pj.ms, powerio_jl_std_ms = pj.std_ms, powerio_jl_n = pj.n,
                  powerio_data_ms = data === nothing ? nothing : data.ms,
                  powerio_data_std_ms = data === nothing ? nothing : data.std_ms,
                  powerio_data_n = data === nothing ? 0 : data.n,
                  rust_c_abi_ms = rust_c.ms, rust_c_abi_std_ms = rust_c.std_ms,
                  rust_c_abi_n = rust_c.n,
                  exapowerio_ms = e.ms, exapowerio_std_ms = e.std_ms, exapowerio_n = e.n,
                  powermodels_ms = pm === nothing ? nothing : pm.ms,
                  powermodels_std_ms = pm === nothing ? nothing : pm.std_ms,
                  powermodels_n = pm === nothing ? 0 : pm.n))
end

println()
println(rpad("case", 20), rpad("PowerIO.jl Ybus", 24), rpad("Exa Ybus", 24),
        rpad("Rust C ABI", 24), rpad("PM Ybus", 24), "nnz rows")
for (name, f, _run_pm_parse, run_pm_ybus) in CASES
    if !isfile(f)
        continue
    end

    h = pio_parse_file(f); nbuses = pio_n_buses(h); nbranch = pio_n_branches(h); pio_free(h)
    samples = nbuses > 30_000 ? 5 : 30

    nnz_rows = powerio_parse_ybus_arrow(f)
    bp = @benchmark PowerIO.calc_admittance_matrix($f) samples = samples evals = 1
    pio = trial_stats(bp)

    braw = @benchmark powerio_parse_ybus_arrow($f) samples = samples evals = 1
    raw = trial_stats(braw)

    be = @benchmark exapowerio_parse_ybus($f) samples = samples evals = 1
    exa = trial_stats(be)

    pm_ybus = nothing
    pm_display = "skip"
    if run_pm_ybus
        try
            bpm = @benchmark powermodels_parse_ybus($f) samples = 5 evals = 1 seconds = 60
            pm_ybus = trial_stats(bpm; digits = 1)
            pm_display = show_stat(pm_ybus)
        catch e
            @warn "PowerModels Ybus benchmark failed" case = name error = sprint(showerror, e)
        end
    end

    println(rpad(name, 20), rpad(show_stat(pio), 24), rpad(show_stat(exa), 24),
            rpad(show_stat(raw), 24), rpad(pm_display, 24), nnz_rows)
    push!(matrix_jrows, (case = name, buses = nbuses, branches = nbranch,
                         powerio_jl_ybus_ms = pio.ms,
                         powerio_jl_ybus_std_ms = pio.std_ms,
                         powerio_jl_ybus_n = pio.n,
                         rust_c_abi_ybus_arrow_ms = raw.ms,
                         rust_c_abi_ybus_arrow_std_ms = raw.std_ms,
                         rust_c_abi_ybus_arrow_n = raw.n,
                         exapowerio_ybus_ms = exa.ms,
                         exapowerio_ybus_std_ms = exa.std_ms,
                         exapowerio_ybus_n = exa.n,
                         powermodels_ybus_ms = pm_ybus === nothing ? nothing : pm_ybus.ms,
                         powermodels_ybus_std_ms = pm_ybus === nothing ? nothing : pm_ybus.std_ms,
                         powermodels_ybus_n = pm_ybus === nothing ? 0 : pm_ybus.n))
end

JSON_OUT && write_speed_julia(joinpath(@__DIR__, "results", "speed_julia.json"), jrows, matrix_jrows)
