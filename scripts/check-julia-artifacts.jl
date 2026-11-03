# Check the packaged release tarballs the way PowerIO.jl will use them, before
# the draft release exists. After publication, PowerIO.jl's Release workflow
# pins the published tarballs in its Artifacts.toml with gen/update_artifacts.jl
# and runs its tests against them. It cannot skip a version, so a tarball that
# fails there leaves PyPI and crates.io published and the Julia release stuck.
#
#   julia --project=PowerIO.jl scripts/check-julia-artifacts.jl pin PowerIO.jl ASSETS vX.Y.Z
#
# runs PowerIO.jl's own gen/update_artifacts.jl on the tarballs in ASSETS
# instead of the release downloads. It inspects the host library (entry
# points, ABI, version string, GridFM parsing) and writes PowerIO.jl's
# Artifacts.toml with the hashes publication will produce.
#
#   julia --project=PowerIO.jl scripts/check-julia-artifacts.jl install PowerIO.jl ASSETS
#
# installs this platform's tarball from ASSETS into the depot under the
# git-tree-sha1 that Artifacts.toml names, checking its sha256 and tree hash as
# a download from the release would, so `artifact"powerio_capi"` resolves to
# exactly the packaged library when the test suite runs.

using Pkg
using Pkg.Artifacts: artifact_meta, artifact_path, download_artifact
using Base: SHA1

function pin(updater, assets, tag)
    fetch_local(_tag, name, dest) = cp(joinpath(assets, name), dest; force=true)
    status = joinpath(mktempdir(), "update-status.toml")
    result = updater.run_update(tag, status; fetcher=fetch_local)
    result.status == "ready" || error(
        "gen/update_artifacts.jl refuses these tarballs ($(result.reason)): $(result.detail)")
    println("PowerIO.jl accepts the $tag tarballs; Artifacts.toml pins them")
end

function install(julia_root, assets)
    toml = joinpath(julia_root, "Artifacts.toml")
    meta = artifact_meta("powerio_capi", toml)
    meta === nothing && error("$toml has no powerio_capi entry for $(Base.BinaryPlatforms.triplet(Base.BinaryPlatforms.HostPlatform()))")
    download = only(meta["download"])
    tarball = abspath(joinpath(assets, basename(download["url"])))
    isfile(tarball) || error("no packaged tarball at $tarball")
    url = "file://" * (Sys.iswindows() ? "/" * replace(tarball, '\\' => '/') : tarball)
    hash = SHA1(meta["git-tree-sha1"])
    result = download_artifact(hash, url, download["sha256"])
    result === true || error("installing $(basename(tarball)) failed: $result")
    lib = joinpath(artifact_path(hash), Sys.iswindows() ? "bin" : "lib",
                   "libpowerio_capi.$(Base.Libc.Libdl.dlext)")
    isfile(lib) || error("the installed artifact has no $lib")
    println("artifact powerio_capi resolves to ", lib)
end

if length(ARGS) == 4 && ARGS[1] == "pin"
    # Included at top level so the updater module exists in the world `pin` runs in.
    include(joinpath(abspath(ARGS[2]), "gen", "update_artifacts.jl"))
    pin(PowerIOArtifactUpdater, abspath(ARGS[3]), ARGS[4])
elseif length(ARGS) == 3 && ARGS[1] == "install"
    install(abspath(ARGS[2]), abspath(ARGS[3]))
else
    error("usage: check-julia-artifacts.jl pin JULIA_ROOT ASSETS vX.Y.Z | install JULIA_ROOT ASSETS")
end
