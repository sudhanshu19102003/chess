#!/bin/sh
# Builds the release archive for one target: dist/chess-<target>.tar.gz
# (.zip for Windows), holding chess-tui, the two servers (chess-rendezvous,
# chess-relay) and the README. Used by .github/workflows/release.yml; runs
# locally too (Windows targets need a Windows machine with Visual Studio's
# C++ build tools, or the release workflow).
#
#   scripts/package.sh aarch64-unknown-linux-musl
#
# Linux targets are linked with rust-lld and musl's own C runtime, so they are
# static and need no C toolchain, which also lets a Mac build them. Windows
# targets link the C runtime statically, so no Visual C++ redistributable is
# needed.

set -eu

target=${1:?usage: scripts/package.sh TARGET}
cd "$(dirname "$0")/.."

case "$target" in
    *-linux-musl)
        var=CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER
        export "$var=rust-lld"
        ;;
    *-windows-msvc)
        export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
        ;;
esac
exe=
case "$target" in *-windows-*) exe=.exe ;; esac

rustup target add "$target" >/dev/null 2>&1 || true
cargo build --locked --profile dist --target "$target" \
    -p chess-tui --bin chess-tui -p chess-net --bin chess-rendezvous --bin chess-relay

name="chess-$target"
stage="target/package/$name"
rm -rf "$stage"
mkdir -p "$stage" dist
for bin in chess-tui chess-rendezvous chess-relay; do
    cp "target/$target/dist/$bin$exe" "$stage/"
done
cp README.md "$stage/"
if [ -n "$exe" ]; then
    # Windows unpacks zip files itself. Git Bash has no zip, but GitHub's
    # Windows runners have 7-Zip.
    rm -f "dist/$name.zip"
    if command -v zip >/dev/null 2>&1; then
        (cd target/package && zip -qr "../../dist/$name.zip" "$name")
    else
        (cd target/package && 7z a -tzip -bd "../../dist/$name.zip" "$name" >/dev/null)
    fi
    echo "dist/$name.zip"
else
    # Without --no-xattrs, macOS adds headers that GNU tar warns about.
    tar --no-xattrs -czf "dist/$name.tar.gz" -C target/package "$name"
    echo "dist/$name.tar.gz"
fi
