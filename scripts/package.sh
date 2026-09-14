#!/bin/sh
# Build and package release artifacts for raemote.
#
#   scripts/package.sh                      # package the host target
#   scripts/package.sh aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu
#
# Output (in ./dist):
#   raemote-<os>-<arch>.tar.gz   (contains raemote + raemoted)
#   checksums.txt                (sha256 of each tarball)
#
# Upload the tarballs (and checksums.txt) as release assets so install.sh can
# fetch them. The GitHub Actions workflow does this automatically; this script
# is for manual/local builds (e.g. on a Linux box).
set -eu

cd "$(dirname "$0")/.."

targets="$*"
if [ -z "$targets" ]; then
    targets=$(rustc -vV | awk '/^host:/ {print $2}')
fi

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

mkdir -p dist
: > dist/checksums.txt

for target in $targets; do
    case "$target" in
        aarch64-apple-darwin)      os=darwin; arch=arm64 ;;
        x86_64-apple-darwin)       os=darwin; arch=x86_64 ;;
        x86_64-unknown-linux-gnu)  os=linux;  arch=x86_64 ;;
        aarch64-unknown-linux-gnu) os=linux;  arch=arm64 ;;
        *) echo "unsupported target: $target" >&2; exit 1 ;;
    esac
    asset="raemote-$os-$arch"

    echo ">> building $target"
    rustup target add "$target" >/dev/null 2>&1 || true
    cargo build --release --target "$target"

    staging=$(mktemp -d 2>/dev/null || mktemp -d -t raemote)
    cp "target/$target/release/raemote" "$staging/"
    cp "target/$target/release/raemoted" "$staging/"
    tar -C "$staging" -czf "dist/$asset.tar.gz" raemote raemoted
    rm -rf "$staging"

    printf '%s  %s\n' "$(sha256 "dist/$asset.tar.gz")" "$asset.tar.gz" >> dist/checksums.txt
    echo "   dist/$asset.tar.gz"
done

echo ">> wrote dist/checksums.txt"
