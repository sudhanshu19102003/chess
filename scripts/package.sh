#!/bin/sh
# Builds the release archive for one target: dist/chess-<target>.tar.gz,
# holding chess-tui, the two servers (chess-rendezvous, chess-relay) and the
# README. Used by .github/workflows/release.yml; runs locally too.
#
#   scripts/package.sh aarch64-unknown-linux-musl
#
# Linux targets are linked with rust-lld and musl's own C runtime, so they are
# static and need no C toolchain, which also lets a Mac build them.

set -eu

target=${1:?usage: scripts/package.sh TARGET}
cd "$(dirname "$0")/.."

case "$target" in
    *-linux-musl)
        var=CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER
        export "$var=rust-lld"
        ;;
esac

rustup target add "$target" >/dev/null 2>&1 || true
cargo build --locked --profile dist --target "$target" \
    -p chess-tui --bin chess-tui -p chess-net --bin chess-rendezvous --bin chess-relay

name="chess-$target"
stage="target/package/$name"
rm -rf "$stage"
mkdir -p "$stage" dist
for bin in chess-tui chess-rendezvous chess-relay; do
    cp "target/$target/dist/$bin" "$stage/"
done
cp README.md "$stage/"
# Without --no-xattrs, macOS adds headers that GNU tar warns about.
tar --no-xattrs -czf "dist/$name.tar.gz" -C target/package "$name"
echo "dist/$name.tar.gz"
