#!/usr/bin/env bash
# Publish the first npm version of powerio-wasm from a maintainer's machine.
#
# npm accepts a trusted publisher only for a package that exists, so the npm
# workflow (.github/workflows/npm.yml) cannot publish the first version. Run
# this once, after the release is published and the workflow's build job has
# passed; the workflow's publish job fails with a message naming this script
# until the package exists. Later releases publish through the workflow.
#
#   scripts/npm-bootstrap.sh vX.Y.Z           # the tarball the workflow built
#   scripts/npm-bootstrap.sh vX.Y.Z --build   # build it here from the tag
#
# It uses your GitHub CLI login and your npm login (`npm login`), checks that
# the tarball is powerio-wasm at the tag's version, publishes it, and prints
# the trusted publisher settings to enter on npmjs.com.
set -euo pipefail

REPO=eigenergy/powerio
PACKAGE=powerio-wasm
ARTIFACT=powerio-wasm-npm

usage() { echo "usage: $0 vX.Y.Z [--build]" >&2; exit 2; }
fail() { echo "npm-bootstrap: $*" >&2; exit 1; }

[ $# -ge 1 ] && [ $# -le 2 ] || usage
tag=$1
build=false
case "${2:-}" in
    "") ;;
    --build) build=true ;;
    *) usage ;;
esac
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "$tag is not a vX.Y.Z release tag"
version=${tag#v}

for tool in gh npm jq tar git; do
    command -v "$tool" >/dev/null || fail "$tool is required"
done
$build && { command -v wasm-pack >/dev/null || fail "--build requires wasm-pack 0.15.0"; }

gh api "repos/$REPO/releases/tags/$tag" --jq '.draft == false and .prerelease == false' \
    | grep -qx true || fail "$tag is not a published stable GitHub release"
tag_commit=$(gh api "repos/$REPO/commits/$tag" --jq .sha) || fail "cannot resolve $tag on GitHub"

# A first publication only: an existing package, ours or someone else's, is
# never published from here.
if out=$(npm view "$PACKAGE" name 2>&1); then
    fail "$PACKAGE already exists on npm. Configure its trusted publisher if it has none, then re-run the npm workflow for $tag."
elif ! grep -q E404 <<<"$out"; then
    fail "npm view $PACKAGE failed: $out"
fi
user=$(npm whoami 2>/dev/null) || fail "not logged in to npm; run npm login first"

work=$(mktemp -d)
worktree=
# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() {
    if [ -n "$worktree" ]; then
        git worktree remove --force "$worktree" >/dev/null 2>&1 || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

if $build; then
    root=$(git rev-parse --show-toplevel)
    git -C "$root" fetch --quiet origin "refs/tags/$tag:refs/tags/$tag"
    [ "$(git -C "$root" rev-parse "refs/tags/$tag^{commit}")" = "$tag_commit" ] \
        || fail "the local tag $tag does not name $tag_commit, the commit on GitHub"
    worktree="$work/src"
    git -C "$root" worktree add --quiet --detach "$worktree" "$tag_commit"
    (cd "$worktree" && cargo test --locked -p powerio-wasm)
    [ -f "$worktree/scripts/build-npm-package.sh" ] \
        || fail "$tag predates scripts/build-npm-package.sh; it cannot be published to npm"
    bash "$worktree/scripts/build-npm-package.sh"
    mkdir "$work/dist"
    mv "$worktree/dist/$PACKAGE-$version.tgz" "$work/dist/"
else
    run=$(gh run list -R "$REPO" --workflow npm.yml --limit 100 \
            --json databaseId,displayTitle,headSha \
            --jq "[.[] | select(.displayTitle == \"PowerIO npm $tag\" and .headSha == \"$tag_commit\") | .databaseId] | max // empty")
    [ -n "$run" ] || fail "no npm workflow run for $tag; wait for it, or pass --build"
    gh run download -R "$REPO" "$run" -n "$ARTIFACT" -D "$work/dist" \
        || fail "run $run has no $ARTIFACT artifact (its build job must pass); or pass --build"
fi

tarball="$work/dist/$PACKAGE-$version.tgz"
[ -f "$tarball" ] || fail "no $PACKAGE-$version.tgz in the build output"
tar -xzOf "$tarball" package/package.json \
    | jq -e --arg name "$PACKAGE" --arg version "$version" '
        .name == $name and .version == $version and .license == "MIT OR Apache-2.0"' >/dev/null \
    || fail "the tarball is not $PACKAGE $version under MIT OR Apache-2.0"
contents=$(tar -tzf "$tarball")
for file in README.md LICENSE-MIT LICENSE-APACHE powerio_wasm_bg.wasm; do
    grep -qx "package/$file" <<<"$contents" || fail "the tarball has no $file"
done

echo "About to publish $PACKAGE $version to npm as $user from $tag ($tag_commit)."
read -r -p "Type $PACKAGE to publish: " answer
[ "$answer" = "$PACKAGE" ] || fail "not published"
npm publish "$tarball" --access public

for _ in 1 2 3 4 5 6; do
    if [ "$(npm view "$PACKAGE@$version" version 2>/dev/null)" = "$version" ]; then
        cat <<EOF

Published $PACKAGE $version. Now configure its trusted publisher at
https://www.npmjs.com/package/$PACKAGE/access (Trusted Publisher, GitHub Actions):

  Organization or user:  eigenergy
  Repository:            powerio
  Workflow filename:     npm.yml
  Environment name:      npm

Then re-run the failed npm workflow job for $tag; it reports $version as
already published. Later releases publish through the workflow.
EOF
        exit 0
    fi
    sleep 10
done
fail "$PACKAGE $version is not visible on npm a minute after publishing; check npmjs.com"
