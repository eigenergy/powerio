#!/usr/bin/env bash
# Build and check the npm tarball of powerio-wasm: dist/powerio-wasm-X.Y.Z.tgz.
#
# The build is the converter's (apps/converter/scripts/build-wasm.mjs):
# wasm-pack --target web, release profile, default features, so SQLite needs
# a Clang with the wasm32 backend. npm.yml runs this after the parity test,
# and scripts/npm-bootstrap.sh --build runs it from a checkout of the tag.
# It refuses a package whose name, version, license, repository, or contents
# differ from what the release publishes.
set -euo pipefail
cd "$(dirname "$0")/.."

PACKAGE=powerio-wasm
version=$(cargo metadata --no-deps --format-version 1 --locked \
    | jq -er '.packages[] | select(.name == "powerio") | .version')
crate_version=$(cargo metadata --no-deps --format-version 1 --locked \
    | jq -er '.packages[] | select(.name == "powerio-wasm") | .version')
[ "$crate_version" = "$version" ] \
    || { echo "powerio-wasm is $crate_version, the workspace is $version" >&2; exit 1; }

pkg=powerio-wasm/pkg
rm -rf "$pkg"
# Arguments after --locked go to cargo, so it stays last.
CARGO_INCREMENTAL=0 wasm-pack build powerio-wasm --target web --out-dir pkg --release --locked

# wasm-pack copies the crate's license files into the package but leaves them
# out of package.json "files", and npm packs a license file by itself only
# when it is named LICENSE or LICENCE.
manifest=$pkg/package.json
jq '.files += (["LICENSE-MIT", "LICENSE-APACHE"] - .files)' "$manifest" > "$manifest.tmp"
mv "$manifest.tmp" "$manifest"
jq -e --arg name "$PACKAGE" --arg version "$version" '
    .name == $name
    and .version == $version
    and .license == "MIT OR Apache-2.0"
    and .repository.url == "https://github.com/eigenergy/powerio"
' "$manifest" >/dev/null || {
    echo "$manifest does not state $PACKAGE $version with the license and repository the release publishes:" >&2
    cat "$manifest" >&2
    exit 1
}

wasm-pack pack powerio-wasm
tarball=$pkg/$PACKAGE-$version.tgz
[ -f "$tarball" ] || { echo "wasm-pack pack did not write $tarball" >&2; exit 1; }
contents=$(tar -tzf "$tarball")
printf '%s\n' "$contents"
for file in package.json README.md LICENSE-MIT LICENSE-APACHE \
            powerio_wasm.js powerio_wasm.d.ts powerio_wasm_bg.wasm; do
    grep -qx "package/$file" <<<"$contents" \
        || { echo "$tarball does not contain $file" >&2; exit 1; }
done
tar -xzOf "$tarball" package/package.json \
    | jq -e --arg version "$version" '.version == $version' >/dev/null \
    || { echo "$tarball does not carry version $version" >&2; exit 1; }

mkdir -p dist
mv "$tarball" dist/
echo "built dist/$PACKAGE-$version.tgz"
