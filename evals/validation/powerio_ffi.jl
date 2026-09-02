# Thin Julia binding over the powerio C ABI 7 (powerio-capi), shared by the
# parse benchmark and the ExaPowerIO validator. Build the library first:
#
#   cargo build --release -p powerio-capi
#
# ABI 7 exposes one balanced network through owner rooted typed views: a
# `pio_balanced_network_*_count` query and a `pio_balanced_network_*_at` fill
# per element family. The views carry raw MATPOWER values: demand, shunt, and
# generation in MW/MVAr (not per unit), branch `shift` in degrees, branch `b`
# as the total line charging, and a raw `tap` that may be 0 (meaning 1). Dense
# bus index == file order, so row k of every per-bus table lines up with
# bus_ids[k]. Struct layouts mirror powerio-capi/include/powerio.h field for
# field; a header change that reorders a view must be mirrored here.

using Libdl

const _LIBEXT = Sys.iswindows() ? "dll" : (Sys.isapple() ? "dylib" : "so")
const LIBPOWERIO = abspath(joinpath(@__DIR__, "..", "..", "target", "release", "libpowerio_capi.$_LIBEXT"))

isfile(LIBPOWERIO) || error("libpowerio_capi not found at $LIBPOWERIO — run `cargo build --release -p powerio-capi`")

# One structured error read: code and message off the PioError handle, then
# release it.
function _take_error(err::Ptr{Cvoid})
    err == C_NULL && return "(no error detail)"
    code = unsafe_string(ccall((:pio_error_code, LIBPOWERIO), Cstring, (Ptr{Cvoid},), err))
    message = unsafe_string(ccall((:pio_error_message, LIBPOWERIO), Cstring, (Ptr{Cvoid},), err))
    ccall((:pio_error_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), err)
    return "$code: $message"
end

# --- header view structs -------------------------------------------------

struct PioStringView
    data::Ptr{UInt8}
    len::Csize_t
end

struct PioF64View
    data::Ptr{Float64}
    len::Csize_t
end

struct PioComponentIdView
    component_type::PioStringView
    local_id::PioStringView
end

struct PioTerminalReferenceView
    equipment::PioComponentIdView
    terminal::UInt8
end

struct PioBalancedLocationView
    x::Float64
    y::Float64
    kind::PioStringView
    has_kind::Bool
end

struct PioBalancedBusView
    component_id::PioStringView
    has_component_id::Bool
    id::Csize_t
    bus_type::PioStringView
    vm_pu::Float64
    va_degrees::Float64
    base_kv::Float64
    vmax_pu::Float64
    vmin_pu::Float64
    has_emergency_voltage_limits::Bool
    emergency_vmax_pu::Float64
    emergency_vmin_pu::Float64
    area::Csize_t
    zone::Csize_t
    name::PioStringView
    has_name::Bool
    location::PioBalancedLocationView
    has_location::Bool
end

struct PioBalancedLoadVoltageModelView
    kind::PioStringView
    p_constant_power_mw::Float64
    q_constant_power_mvar::Float64
    p_constant_current_mw::Float64
    q_constant_current_mvar::Float64
    p_constant_impedance_mw::Float64
    q_constant_impedance_mvar::Float64
    exponential_p_mw::Float64
    exponential_q_mvar::Float64
    gamma_p::Float64
    gamma_q::Float64
    nominal_voltage_pu::Float64
    has_nominal_voltage::Bool
    load_type::Int32
    has_load_type::Bool
    scaling::Float64
    has_scaling::Bool
end

struct PioBalancedLoadView
    component_id::PioStringView
    has_component_id::Bool
    bus_id::Csize_t
    p_mw::Float64
    q_mvar::Float64
    in_service::Bool
    voltage_model::PioBalancedLoadVoltageModelView
end

struct PioBalancedShuntView
    component_id::PioStringView
    has_component_id::Bool
    bus_id::Csize_t
    conductance_mw::Float64
    susceptance_mvar::Float64
    in_service::Bool
    section_count::UInt32
    has_section_count::Bool
    has_control::Bool
    control_mode::PioStringView
    control_vmax_pu::Float64
    control_vmin_pu::Float64
    control_bus_id::Csize_t
    has_control_bus::Bool
    control_reactive_range_percent::Float64
    control_block_count::Csize_t
end

struct PioTransformerControlView
    mode::PioStringView
    enabled::Bool
    controlled_bus_id::Csize_t
    has_controlled_bus::Bool
    controlled_bus_on_winding_side::Bool
    regulating_terminal::PioTerminalReferenceView
    has_regulating_terminal::Bool
    tap_min::Float64
    tap_max::Float64
    band_min::Float64
    band_max::Float64
    tap_position_count::UInt32
    mva_base::Float64
    winding_connection_angle::Float64
    has_winding_connection_angle::Bool
end

struct PioBalancedBranchView
    component_id::PioStringView
    has_component_id::Bool
    name::PioStringView
    has_name::Bool
    from_bus_id::Csize_t
    to_bus_id::Csize_t
    resistance_pu::Float64
    reactance_pu::Float64
    total_charging_susceptance_pu::Float64
    terminal_charging_is_explicit::Bool
    from_conductance_pu::Float64
    from_susceptance_pu::Float64
    to_conductance_pu::Float64
    to_susceptance_pu::Float64
    rate_a_mva::Float64
    rate_b_mva::Float64
    rate_c_mva::Float64
    additional_rating_count::Csize_t
    has_current_ratings::Bool
    current_rating_a::Float64
    current_rating_b::Float64
    current_rating_c::Float64
    tap_ratio::Float64
    effective_tap_ratio::Float64
    phase_shift_degrees::Float64
    in_service::Bool
    angle_min_degrees::Float64
    angle_max_degrees::Float64
    control::PioTransformerControlView
    has_control::Bool
    route_point_count::Csize_t
    has_route::Bool
end

struct PioGeneratorCostView
    model::UInt8
    startup::Float64
    shutdown::Float64
    ncost::Csize_t
    coefficients::PioF64View
end

struct PioActivePowerControlView
    participate::Bool
    droop_percent::Float64
    has_droop_percent::Bool
    participation_factor::Float64
    has_participation_factor::Bool
    minimum_target_active_power_mw::Float64
    has_minimum_target_active_power::Bool
    maximum_target_active_power_mw::Float64
    has_maximum_target_active_power::Bool
end

struct PioBalancedGeneratorView
    component_id::PioStringView
    has_component_id::Bool
    bus_id::Csize_t
    energy_source::PioStringView
    active_power_mw::Float64
    reactive_power_mvar::Float64
    active_power_max_mw::Float64
    active_power_min_mw::Float64
    reactive_power_max_mvar::Float64
    reactive_power_min_mvar::Float64
    voltage_setpoint_pu::Float64
    machine_base_mva::Float64
    in_service::Bool
    has_cost::Bool
    cost::PioGeneratorCostView
    regulated_bus_id::Csize_t
    has_regulated_bus::Bool
    capability_count::Csize_t
    active_power_control::PioActivePowerControlView
    has_active_power_control::Bool
    voltage_regulation_on::Bool
    regulating_terminal::PioTerminalReferenceView
    has_regulating_terminal::Bool
end

# --- parse ---------------------------------------------------------------

# Parse `path` into a balanced network handle: open the source, parse it to a
# module, borrow the value as a balanced network, and release the module and
# source. The network handle is reference counted and outlives both.
function powerio_parse_balanced(path::AbstractString)
    err = Ref{Ptr{Cvoid}}(C_NULL)
    source = ccall((:pio_source_open, LIBPOWERIO), Ptr{Cvoid},
                   (Ptr{UInt8}, Csize_t, Ref{Ptr{Cvoid}}), path, sizeof(path), err)
    source == C_NULL && error("powerio could not open $path: " * _take_error(err[]))
    m = ccall((:pio_parse, LIBPOWERIO), Ptr{Cvoid},
              (Ptr{Cvoid}, Ptr{UInt8}, Csize_t, Ref{Ptr{Cvoid}}), source, C_NULL, 0, err)
    ccall((:pio_source_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), source)
    m == C_NULL && error("powerio parse failed for $path: " * _take_error(err[]))
    value = ccall((:pio_module_value, LIBPOWERIO), Ptr{Cvoid}, (Ptr{Cvoid},), m)
    h = ccall((:pio_value_balanced_network, LIBPOWERIO), Ptr{Cvoid},
              (Ptr{Cvoid}, Ref{Ptr{Cvoid}}), value, err)
    ccall((:pio_value_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), value)
    ccall((:pio_module_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), m)
    h == C_NULL && error("powerio parse of $path holds no balanced network: " * _take_error(err[]))
    return h
end

powerio_release!(h::Ptr{Cvoid}) = ccall((:pio_balanced_network_release, LIBPOWERIO), Cvoid, (Ptr{Cvoid},), h)

powerio_bus_count(h)       = Int(ccall((:pio_balanced_network_bus_count, LIBPOWERIO),       Csize_t, (Ptr{Cvoid},), h))
powerio_branch_count(h)    = Int(ccall((:pio_balanced_network_branch_count, LIBPOWERIO),    Csize_t, (Ptr{Cvoid},), h))
powerio_generator_count(h) = Int(ccall((:pio_balanced_network_generator_count, LIBPOWERIO), Csize_t, (Ptr{Cvoid},), h))
powerio_load_count(h)      = Int(ccall((:pio_balanced_network_load_count, LIBPOWERIO),      Csize_t, (Ptr{Cvoid},), h))
powerio_shunt_count(h)     = Int(ccall((:pio_balanced_network_shunt_count, LIBPOWERIO),     Csize_t, (Ptr{Cvoid},), h))
powerio_base_mva(h)        = ccall((:pio_balanced_network_base_mva, LIBPOWERIO),            Cdouble, (Ptr{Cvoid},), h)

# One typed view fill. `symbol` names the `pio_balanced_network_*_at` entry
# point and `T` the matching view struct; the index is zero based. `ccall`
# takes a literal symbol or a function pointer, so the entry point is
# resolved through `dlsym` once per call.
const _LIBHANDLE = Libdl.dlopen(LIBPOWERIO)

function _view_at(::Type{T}, symbol::Symbol, h::Ptr{Cvoid}, index::Integer) where {T}
    out = Ref{T}()
    err = Ref{Ptr{Cvoid}}(C_NULL)
    ok = ccall(Libdl.dlsym(_LIBHANDLE, symbol), Bool,
               (Ptr{Cvoid}, Csize_t, Ref{T}, Ref{Ptr{Cvoid}}), h, index, out, err)
    ok || error("powerio $symbol($index) failed: " * _take_error(err[]))
    return out[]
end

powerio_bus_at(h, i)       = _view_at(PioBalancedBusView,       :pio_balanced_network_bus_at,       h, i)
powerio_branch_at(h, i)    = _view_at(PioBalancedBranchView,    :pio_balanced_network_branch_at,    h, i)
powerio_generator_at(h, i) = _view_at(PioBalancedGeneratorView, :pio_balanced_network_generator_at, h, i)
powerio_load_at(h, i)      = _view_at(PioBalancedLoadView,      :pio_balanced_network_load_at,      h, i)
powerio_shunt_at(h, i)     = _view_at(PioBalancedShuntView,     :pio_balanced_network_shunt_at,     h, i)

# --- table extractors ----------------------------------------------------

function powerio_bus_ids(h, n)
    ids = Vector{Int64}(undef, n)
    for k in 1:n
        ids[k] = Int64(powerio_bus_at(h, k - 1).id)
    end
    ids
end

function powerio_branches(h, m)
    from  = Vector{Int64}(undef, m); to = Vector{Int64}(undef, m)
    r     = Vector{Float64}(undef, m); x = Vector{Float64}(undef, m)
    b     = Vector{Float64}(undef, m); tap = Vector{Float64}(undef, m)
    shift = Vector{Float64}(undef, m); insvc = Vector{UInt8}(undef, m)
    for k in 1:m
        v = powerio_branch_at(h, k - 1)
        from[k] = Int64(v.from_bus_id); to[k] = Int64(v.to_bus_id)
        r[k] = v.resistance_pu; x[k] = v.reactance_pu
        b[k] = v.total_charging_susceptance_pu; tap[k] = v.tap_ratio
        shift[k] = v.phase_shift_degrees; insvc[k] = UInt8(v.in_service)
    end
    (; from, to, r, x, b, tap, shift, in_service = insvc)
end

function powerio_generators(h, ng)
    bus  = Vector{Int64}(undef, ng); pg = Vector{Float64}(undef, ng)
    pmax = Vector{Float64}(undef, ng); pmin = Vector{Float64}(undef, ng)
    insvc = Vector{UInt8}(undef, ng)
    for k in 1:ng
        v = powerio_generator_at(h, k - 1)
        bus[k] = Int64(v.bus_id); pg[k] = v.active_power_mw
        pmax[k] = v.active_power_max_mw; pmin[k] = v.active_power_min_mw
        insvc[k] = UInt8(v.in_service)
    end
    (; bus, pg, pmax, pmin, in_service = insvc)
end

# Per-bus demand: in-service loads summed onto their bus in bus_ids order,
# which is the MATPOWER bus table's PD/QD. An out of service load contributes
# nothing, matching the MATPOWER writer.
function powerio_bus_demand(h, bus_ids::Vector{Int64})
    n = length(bus_ids)
    row = Dict{Int64,Int}(id => k for (k, id) in enumerate(bus_ids))
    pd = zeros(Float64, n); qd = zeros(Float64, n)
    for k in 1:powerio_load_count(h)
        v = powerio_load_at(h, k - 1)
        v.in_service || continue
        i = row[Int64(v.bus_id)]
        pd[i] += v.p_mw; qd[i] += v.q_mvar
    end
    (; pd, qd)
end

# Per-bus shunt: in-service shunts summed onto their bus, the MATPOWER GS/BS.
function powerio_bus_shunt(h, bus_ids::Vector{Int64})
    n = length(bus_ids)
    row = Dict{Int64,Int}(id => k for (k, id) in enumerate(bus_ids))
    gs = zeros(Float64, n); bs = zeros(Float64, n)
    for k in 1:powerio_shunt_count(h)
        v = powerio_shunt_at(h, k - 1)
        v.in_service || continue
        i = row[Int64(v.bus_id)]
        gs[i] += v.conductance_mw; bs[i] += v.susceptance_mvar
    end
    (; gs, bs)
end

# Parse and extract every table into one NamedTuple, then release the handle.
function powerio_load(path::AbstractString)
    h = powerio_parse_balanced(path)
    try
        n, m, ng = powerio_bus_count(h), powerio_branch_count(h), powerio_generator_count(h)
        bus_ids = powerio_bus_ids(h, n)
        (; base_mva = powerio_base_mva(h),
           bus_ids,
           branch = powerio_branches(h, m),
           gen = powerio_generators(h, ng),
           demand = powerio_bus_demand(h, bus_ids),
           shunt = powerio_bus_shunt(h, bus_ids),
           n, m, ng)
    finally
        powerio_release!(h)
    end
end
