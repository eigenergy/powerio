# Thin Julia binding over the powerio C ABI (powerio-capi), shared by the parse
# benchmark and the ExaPowerIO validator. Build the library first:
#
#   cargo build --release -p powerio-capi --features arrow,matrix
#
# The C ABI returns raw MATPOWER values: demand/shunt/gen in MW/MVAr (not per
# unit), branch `shift` in degrees, branch `b` as the total line charging, and a
# raw `tap` that may be 0 (meaning 1). Dense bus index == file order, so row k of
# every table lines up with bus_ids[k]. See powerio-capi/include/powerio.h.

const _LIBEXT = Sys.iswindows() ? "dll" : (Sys.isapple() ? "dylib" : "so")
const LIBPOWERIO = abspath(joinpath(@__DIR__, "..", "target", "release", "libpowerio_capi.$_LIBEXT"))
const PIO_ARROW_TABLE_YBUS = Int32(15)

isfile(LIBPOWERIO) || error("libpowerio_capi not found at $LIBPOWERIO — run `cargo build --release -p powerio-capi --features arrow,matrix`")

# One structured error read: code and message off the PioError handle, then
# release it.
function _take_error(err::Ptr{Cvoid})
    code = unsafe_string(ccall((:pio_error_code, LIBPOWERIO), Cstring, (Ptr{Cvoid},), err))
    message = unsafe_string(ccall((:pio_error_message, LIBPOWERIO), Cstring, (Ptr{Cvoid},), err))
    ccall((:pio_error_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), err)
    return "$code: $message"
end

# Parse `path` into a balanced network handle through the module surface:
# parse to the module, take the balanced network out, release the module.
function pio_parse_file(path::AbstractString)
    err = Ref{Ptr{Cvoid}}(C_NULL)
    m = ccall((:pio_parse_file, LIBPOWERIO), Ptr{Cvoid},
              (Cstring, Cstring, Ref{Ptr{Cvoid}}), path, C_NULL, err)
    m == C_NULL && error("powerio parse failed for $path: " * _take_error(err[]))
    h = ccall((:pio_module_balanced_network, LIBPOWERIO), Ptr{Cvoid},
              (Ptr{Cvoid}, Ref{Ptr{Cvoid}}), m, err)
    ccall((:pio_module_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), m)
    h == C_NULL && error("powerio parse of $path holds no balanced network: " * _take_error(err[]))
    return h
end

pio_free(h::Ptr{Cvoid}) = ccall((:pio_balanced_network_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), h)

pio_n_buses(h)    = Int(ccall((:pio_balanced_network_n_buses, LIBPOWERIO),    Csize_t, (Ptr{Cvoid},), h))
pio_n_branches(h) = Int(ccall((:pio_balanced_network_n_branches, LIBPOWERIO), Csize_t, (Ptr{Cvoid},), h))
pio_n_gens(h)     = Int(ccall((:pio_balanced_network_n_gens, LIBPOWERIO),     Csize_t, (Ptr{Cvoid},), h))
pio_base_mva(h)   = ccall((:pio_balanced_network_base_mva, LIBPOWERIO),       Cdouble, (Ptr{Cvoid},), h)

mutable struct CArrowArray
    length::Int64
    null_count::Int64
    offset::Int64
    n_buffers::Int64
    n_children::Int64
    buffers::Ptr{Ptr{Cvoid}}
    children::Ptr{Ptr{Cvoid}}
    dictionary::Ptr{Cvoid}
    release::Ptr{Cvoid}
    private_data::Ptr{Cvoid}
end

mutable struct CArrowSchema
    format::Ptr{Cchar}
    name::Ptr{Cchar}
    metadata::Ptr{Cchar}
    flags::Int64
    n_children::Int64
    children::Ptr{Ptr{Cvoid}}
    dictionary::Ptr{Cvoid}
    release::Ptr{Cvoid}
    private_data::Ptr{Cvoid}
end

CArrowArray() = CArrowArray(0, 0, 0, 0, 0, Ptr{Ptr{Cvoid}}(C_NULL),
                            Ptr{Ptr{Cvoid}}(C_NULL), C_NULL, C_NULL, C_NULL)
CArrowSchema() = CArrowSchema(C_NULL, C_NULL, C_NULL, 0, 0,
                              Ptr{Ptr{Cvoid}}(C_NULL), C_NULL, C_NULL, C_NULL)

function _release_arrow_array!(arr::Base.RefValue{CArrowArray})
    release = arr[].release
    release == C_NULL || ccall(release, Cvoid, (Ref{CArrowArray},), arr)
    return nothing
end

function _release_arrow_schema!(sch::Base.RefValue{CArrowSchema})
    release = sch[].release
    release == C_NULL || ccall(release, Cvoid, (Ref{CArrowSchema},), sch)
    return nothing
end

function pio_export_ybus_arrow(h)
    arr = Ref(CArrowArray())
    sch = Ref(CArrowSchema())
    err = Ref{Ptr{Cvoid}}(C_NULL)
    code = ccall((:pio_balanced_network_to_arrow, LIBPOWERIO), Cint,
                 (Ptr{Cvoid}, Cint, Ref{CArrowArray}, Ref{CArrowSchema}, Ref{Ptr{Cvoid}}),
                 h, PIO_ARROW_TABLE_YBUS, arr, sch, err)
    code == 0 || error("powerio Ybus Arrow export failed: " * _take_error(err[]))
    try
        return Int(arr[].length)
    finally
        _release_arrow_array!(arr)
        _release_arrow_schema!(sch)
    end
end

function powerio_parse_ybus_arrow(path::AbstractString)
    h = pio_parse_file(path)
    try
        return pio_export_ybus_arrow(h)
    finally
        pio_free(h)
    end
end

# Caller fill extractors: every array call passes a cap and returns the total
# available (NULL out is the count query). The caps below come from the
# matching pio_n_* call, so the returned totals always equal them.

function pio_bus_ids(h, n)
    out = Vector{Int64}(undef, n)
    ccall((:pio_balanced_network_bus_ids, LIBPOWERIO), Csize_t, (Ptr{Cvoid}, Ptr{Int64}, Csize_t), h, out, n)
    out
end

function pio_branches(h, m)
    from  = Vector{Int64}(undef, m); to = Vector{Int64}(undef, m)
    r     = Vector{Float64}(undef, m); x = Vector{Float64}(undef, m)
    b     = Vector{Float64}(undef, m); tap = Vector{Float64}(undef, m)
    shift = Vector{Float64}(undef, m); insvc = Vector{UInt8}(undef, m)
    ccall((:pio_balanced_network_branches, LIBPOWERIO), Csize_t,
          (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
           Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}, Csize_t),
          h, from, to, r, x, b, tap, shift, insvc, m)
    (; from, to, r, x, b, tap, shift, in_service = insvc)
end

function pio_gens(h, ng)
    bus  = Vector{Int64}(undef, ng); pg = Vector{Float64}(undef, ng)
    pmax = Vector{Float64}(undef, ng); pmin = Vector{Float64}(undef, ng)
    insvc = Vector{UInt8}(undef, ng)
    ccall((:pio_balanced_network_gens, LIBPOWERIO), Csize_t,
          (Ptr{Cvoid}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}, Csize_t),
          h, bus, pg, pmax, pmin, insvc, ng)
    (; bus, pg, pmax, pmin, in_service = insvc)
end

function pio_bus_demand(h, n)
    pd = Vector{Float64}(undef, n); qd = Vector{Float64}(undef, n)
    ccall((:pio_balanced_network_bus_demand, LIBPOWERIO), Csize_t,
          (Ptr{Cvoid}, Ptr{Float64}, Ptr{Float64}, Csize_t), h, pd, qd, n)
    (; pd, qd)
end

function pio_bus_shunt(h, n)
    gs = Vector{Float64}(undef, n); bs = Vector{Float64}(undef, n)
    ccall((:pio_balanced_network_bus_shunt, LIBPOWERIO), Csize_t,
          (Ptr{Cvoid}, Ptr{Float64}, Ptr{Float64}, Csize_t), h, gs, bs, n)
    (; gs, bs)
end

# Parse and extract every table into one NamedTuple, then free the handle.
function powerio_load(path::AbstractString)
    h = pio_parse_file(path)
    try
        n, m, ng = pio_n_buses(h), pio_n_branches(h), pio_n_gens(h)
        (; base_mva = pio_base_mva(h),
           bus_ids = pio_bus_ids(h, n),
           branch = pio_branches(h, m),
           gen = pio_gens(h, ng),
           demand = pio_bus_demand(h, n),
           shunt = pio_bus_shunt(h, n),
           n, m, ng)
    finally
        pio_free(h)
    end
end
