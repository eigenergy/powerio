# Shared checks for the PowerModelsACDC MATPOWER-extension validation.
module AcDcValidation
using PowerModelsACDC
const PM = PowerModelsACDC._PM
export parse_case, compare_cases, case_projection, PM

function parse_case(path)
    data = PM.parse_file(path)
    for (old, new) in [("dcbus", "busdc"), ("dcconv", "convdc"), ("dcbranch", "branchdc")]
        if haskey(data, old)
            @assert !haskey(data, new) "conflicting aliases: $old/$new"
            data[new] = pop!(data, old)
        end
    end
    return data
end

# No electrical tables or input fields are whitelisted away. These exclusions
# concern provenance and previous solve results, never starts, controls or limits.
function excluded(path)
    length(path) == 1 && return path[1] in ("name", "source_type", "source_version")
    length(path) != 3 && return false
    table, _, field = path
    field == "source_id" && return true
    duals = Dict(
        "bus" => ("lam_p", "lam_q", "mu_vmax", "mu_vmin"),
        "gen" => ("mu_pmax", "mu_pmin", "mu_qmax", "mu_qmin"),
        "branch" => ("mu_sf", "mu_st", "mu_angmin", "mu_angmax"),
    )
    return field in get(duals, table, ())
end

function case_projection(value, path=String[]; omitted=Dict{String,Any}())
    if value isa AbstractDict
        result = Dict{String,Any}()
        for (key, child) in value
            child_path = [path; key]
            if excluded(child_path)
                omitted[join(child_path, "/")] = child
            else
                result[key] = case_projection(child, child_path; omitted)
            end
        end
        return result
    elseif value isa AbstractVector
        return [case_projection(child, [path; string(i)]; omitted) for (i, child) in enumerate(value)]
    end
    return value
end

function compare_values(a, b, path; atol, rtol)
    if a isa AbstractDict && b isa AbstractDict
        @assert Set(keys(a)) == Set(keys(b)) "$path: different fields $(setdiff(keys(a), keys(b))) / $(setdiff(keys(b), keys(a)))"
        for key in keys(a)
            compare_values(a[key], b[key], "$path/$key"; atol, rtol)
        end
    elseif a isa AbstractVector && b isa AbstractVector
        @assert length(a) == length(b) "$path: different lengths"
        for i in eachindex(a)
            compare_values(a[i], b[i], "$path/$i"; atol, rtol)
        end
    elseif a isa Real && b isa Real
        @assert isfinite(a) && isfinite(b) "$path: nonfinite input"
        @assert isapprox(a, b; atol, rtol) "$path: $a != $b"
    else
        @assert isequal(a, b) "$path: $a != $b"
    end
end

function compare_cases(a, b; atol=1e-12, rtol=1e-10)
    left_omitted = Dict{String,Any}()
    right_omitted = Dict{String,Any}()
    left = case_projection(a; omitted=left_omitted)
    right = case_projection(b; omitted=right_omitted)
    compare_values(left, right, "case"; atol, rtol)
    return Dict("atol" => atol, "rtol" => rtol,
        "source_exclusions" => left_omitted, "fresh_exclusions" => right_omitted)
end
end
