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
PowerModels.silence()
include(joinpath(@__DIR__, "powerio_ffi.jl"))
PowerIO.set_library!(get(ENV, "POWERIO_CAPI", LIBPOWERIO))

# (name, path, run_powermodels?) — PowerModels is skipped on the huge cases
# where it takes minutes and the gap is already settled.
const CASES = [
    ("case2869pegase", "tests/data/case2869pegase.m", true),
    ("case_ACTIVSg2000", "tests/data/large/case_ACTIVSg2000.m", true),
    ("case9241pegase", "tests/data/large/case9241pegase.m", true),
    ("case13659pegase", "tests/data/large/case13659pegase.m", true),
    ("case_ACTIVSg10k", "tests/data/large/case_ACTIVSg10k.m", false),
    ("case_ACTIVSg25k", "tests/data/large/case_ACTIVSg25k.m", false),
    ("case_ACTIVSg70k", "tests/data/large/case_ACTIVSg70k.m", false),
    ("case_SyntheticUSA", "tests/data/large/case_SyntheticUSA.m", false),
    ("case99k", "tests/data/large/case99k.m", false),
    ("case193k", "tests/data/large/case193k.m", false),
]

ms(b) = round(median(b).time / 1e6, digits = 2)

# `--json`: also write benchmarks/results/speed_julia.json for render_tables.py.
# Case names are plain ASCII and values are numbers or null, so a hand-rolled
# writer keeps the bench env free of a JSON dependency.
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
        println(io, "{")
        println(io, "  \"rows\": [")
        for (i, r) in enumerate(rows)
            pm = r.powermodels_ms === nothing ? "null" : string(r.powermodels_ms)
            data_ms = r.powerio_data_ms === nothing ? "null" : string(r.powerio_data_ms)
            tail = i < length(rows) ? "," : ""
            println(io, "    {\"case\": \"$(r.case)\", \"buses\": $(r.buses), \"branches\": $(r.branches), " *
                        "\"powerio_jl_ms\": $(r.powerio_jl_ms), " *
                        "\"powerio_data_ms\": $data_ms, " *
                        "\"rust_c_abi_ms\": $(r.rust_c_abi_ms), " *
                        "\"exapowerio_ms\": $(r.exapowerio_ms), " *
                        "\"powermodels_ms\": $pm}$tail")
        end
        println(io, "  ],")
        println(io, "  \"matrix_rows\": [")
        for (i, r) in enumerate(matrix_rows)
            pm = r.powermodels_ybus_ms === nothing ? "null" : string(r.powermodels_ybus_ms)
            tail = i < length(matrix_rows) ? "," : ""
            println(io, "    {\"case\": \"$(r.case)\", \"buses\": $(r.buses), \"branches\": $(r.branches), " *
                        "\"powerio_jl_ybus_ms\": $(r.powerio_jl_ybus_ms), " *
                        "\"rust_c_abi_ybus_arrow_ms\": $(r.rust_c_abi_ybus_arrow_ms), " *
                        "\"exapowerio_ybus_ms\": $(r.exapowerio_ybus_ms), " *
                        "\"powermodels_ybus_ms\": $pm}$tail")
        end
        println(io, "  ]")
        println(io, "}")
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

println(rpad("case", 20), rpad("PowerIO.jl", 13), rpad("ExaPowerIO", 13),
        rpad("PowerModels", 13), rpad("Rust C ABI", 13), rpad("net.data", 13),
        "buses (PowerIO / ExaPowerIO)")
for (name, f, run_pm) in CASES
    if !isfile(f)
        @warn "bench_julia: fixture missing, dropping it from the run" case = name path = f
        continue
    end

    h = pio_parse_file(f); nbuses = pio_n_buses(h); nbranch = pio_n_branches(h); pio_free(h)
    samples = nbuses > 30_000 ? 5 : 30

    netref = Ref{Any}(nothing)
    bj = @benchmark $netref[] = PowerIO.parse_file($f) teardown = (free_network!($netref[]); $netref[] = nothing) samples = samples evals = 1
    pj = ms(bj)

    data_ms = nothing
    data_display = "skip"
    if nbuses <= 15_000
        bd = @benchmark powerio_jl_materialize_data($f) samples = samples evals = 1
        data_ms = ms(bd)
        data_display = "$(data_ms) ms"
    end

    href = Ref{Ptr{Cvoid}}(C_NULL)
    bc = @benchmark $href[] = pio_parse_file($f) teardown = (pio_free($href[])) samples = samples evals = 1
    rust_c = ms(bc)

    ed = exapowerio_parse_matpower(f)
    be = @benchmark exapowerio_parse_matpower($f) samples = samples evals = 1
    e = ms(be)

    pm_ms = nothing
    p = "skip"
    if run_pm
        bp = @benchmark PowerModels.parse_file($f) samples = 5 evals = 1
        pm_ms = round(median(bp).time / 1e6, digits = 1)
        p = "$(pm_ms) ms"
    end

    count = nbuses == length(ed.bus) ? string(nbuses) : "$nbuses / $(length(ed.bus))"
    println(rpad(name, 20), rpad("$(pj) ms", 13), rpad("$(e) ms", 13),
            rpad(p, 13), rpad("$(rust_c) ms", 13), rpad(data_display, 13), count)
    push!(jrows, (case = name, buses = nbuses, branches = nbranch,
                  powerio_jl_ms = pj, powerio_data_ms = data_ms,
                  rust_c_abi_ms = rust_c, exapowerio_ms = e,
                  powermodels_ms = pm_ms))
end

println()
println(rpad("case", 20), rpad("PowerIO.jl Ybus", 18), rpad("Exa Ybus", 15),
        rpad("Rust C ABI", 13), rpad("PM Ybus", 13), "nnz rows")
for (name, f, run_pm) in CASES
    if !isfile(f)
        continue
    end

    h = pio_parse_file(f); nbuses = pio_n_buses(h); nbranch = pio_n_branches(h); pio_free(h)
    samples = nbuses > 30_000 ? 5 : 30

    nnz_rows = powerio_parse_ybus_arrow(f)
    bp = @benchmark PowerIO.calc_admittance_matrix($f) samples = samples evals = 1
    pio_ms = ms(bp)

    braw = @benchmark powerio_parse_ybus_arrow($f) samples = samples evals = 1
    raw_ms = ms(braw)

    be = @benchmark exapowerio_parse_ybus($f) samples = samples evals = 1
    exa_ms = ms(be)

    pm_ybus_ms = nothing
    pm_display = "skip"
    if run_pm
        bpm = @benchmark powermodels_parse_ybus($f) samples = 5 evals = 1
        pm_ybus_ms = round(median(bpm).time / 1e6, digits = 1)
        pm_display = "$(pm_ybus_ms) ms"
    end

    println(rpad(name, 20), rpad("$(pio_ms) ms", 18), rpad("$(exa_ms) ms", 15),
            rpad("$(raw_ms) ms", 13), rpad(pm_display, 13), nnz_rows)
    push!(matrix_jrows, (case = name, buses = nbuses, branches = nbranch,
                         powerio_jl_ybus_ms = pio_ms,
                         rust_c_abi_ybus_arrow_ms = raw_ms,
                         exapowerio_ybus_ms = exa_ms,
                         powermodels_ybus_ms = pm_ybus_ms))
end

JSON_OUT && write_speed_julia(joinpath(@__DIR__, "results", "speed_julia.json"), jrows, matrix_jrows)
