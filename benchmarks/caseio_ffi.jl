# Thin Julia binding over the caseio C ABI (caseio-capi), shared by the parse
# benchmark and the ExaPowerIO validator. Build the library first:
#
#   cargo build --release -p caseio-capi
#
# The C ABI returns raw MATPOWER values: demand/shunt/gen in MW/MVAr (not per
# unit), branch `shift` in degrees, branch `b` as the total line charging, and a
# raw `tap` that may be 0 (meaning 1). Dense bus index == file order, so row k of
# every table lines up with bus_ids[k]. See caseio-capi/include/caseio.h.

const _LIBEXT = Sys.iswindows() ? "dll" : (Sys.isapple() ? "dylib" : "so")
const LIBCASEIO = abspath(joinpath(@__DIR__, "..", "target", "release", "libcaseio_capi.$_LIBEXT"))

isfile(LIBCASEIO) || error("libcaseio_capi not found at $LIBCASEIO — run `cargo build --release -p caseio-capi`")

# Parse `path` (format inferred from the extension); returns an opaque handle.
# Free it with cio_free. Errors with the C message on failure.
function cio_parse(path::AbstractString)
    errbuf = Vector{UInt8}(undef, 256)
    h = ccall((:cio_parse, LIBCASEIO), Ptr{Cvoid},
              (Cstring, Ptr{Cvoid}, Ptr{UInt8}, Csize_t),
              path, C_NULL, errbuf, length(errbuf))
    h == C_NULL && error("caseio parse failed for $path: " * unsafe_string(pointer(errbuf)))
    return h
end

cio_free(h::Ptr{Cvoid}) = ccall((:cio_case_free, LIBCASEIO), Cvoid, (Ptr{Cvoid},), h)

cio_n_buses(h)    = Int(ccall((:cio_n_buses, LIBCASEIO),    Csize_t, (Ptr{Cvoid},), h))
cio_n_branches(h) = Int(ccall((:cio_n_branches, LIBCASEIO), Csize_t, (Ptr{Cvoid},), h))
cio_n_gens(h)     = Int(ccall((:cio_n_gens, LIBCASEIO),     Csize_t, (Ptr{Cvoid},), h))
cio_base_mva(h)   = ccall((:cio_base_mva, LIBCASEIO),       Cdouble, (Ptr{Cvoid},), h)

function cio_bus_ids(h, n)
    out = Vector{Int64}(undef, n)
    ccall((:cio_bus_ids, LIBCASEIO), Cvoid, (Ptr{Cvoid}, Ptr{Int64}), h, out)
    out
end

function cio_branches(h, m)
    from  = Vector{Int64}(undef, m); to = Vector{Int64}(undef, m)
    r     = Vector{Float64}(undef, m); x = Vector{Float64}(undef, m)
    b     = Vector{Float64}(undef, m); tap = Vector{Float64}(undef, m)
    shift = Vector{Float64}(undef, m); insvc = Vector{UInt8}(undef, m)
    ccall((:cio_branches, LIBCASEIO), Cvoid,
          (Ptr{Cvoid}, Ptr{Int64}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64},
           Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}),
          h, from, to, r, x, b, tap, shift, insvc)
    (; from, to, r, x, b, tap, shift, in_service = insvc)
end

function cio_gens(h, ng)
    bus  = Vector{Int64}(undef, ng); pg = Vector{Float64}(undef, ng)
    pmax = Vector{Float64}(undef, ng); pmin = Vector{Float64}(undef, ng)
    insvc = Vector{UInt8}(undef, ng)
    ccall((:cio_gens, LIBCASEIO), Cvoid,
          (Ptr{Cvoid}, Ptr{Int64}, Ptr{Float64}, Ptr{Float64}, Ptr{Float64}, Ptr{UInt8}),
          h, bus, pg, pmax, pmin, insvc)
    (; bus, pg, pmax, pmin, in_service = insvc)
end

function cio_nodal_demand(h, n)
    pd = Vector{Float64}(undef, n); qd = Vector{Float64}(undef, n)
    ccall((:cio_nodal_demand, LIBCASEIO), Cvoid,
          (Ptr{Cvoid}, Ptr{Float64}, Ptr{Float64}), h, pd, qd)
    (; pd, qd)
end

function cio_nodal_shunt(h, n)
    gs = Vector{Float64}(undef, n); bs = Vector{Float64}(undef, n)
    ccall((:cio_nodal_shunt, LIBCASEIO), Cvoid,
          (Ptr{Cvoid}, Ptr{Float64}, Ptr{Float64}), h, gs, bs)
    (; gs, bs)
end

# Parse and extract every table into one NamedTuple, then free the handle.
function caseio_load(path::AbstractString)
    h = cio_parse(path)
    try
        n, m, ng = cio_n_buses(h), cio_n_branches(h), cio_n_gens(h)
        (; base_mva = cio_base_mva(h),
           bus_ids = cio_bus_ids(h, n),
           branch = cio_branches(h, m),
           gen = cio_gens(h, ng),
           demand = cio_nodal_demand(h, n),
           shunt = cio_nodal_shunt(h, n),
           n, m, ng)
    finally
        cio_free(h)
    end
end
