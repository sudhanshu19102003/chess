#!/bin/sh
# Installs chess-tui, the peer-to-peer terminal chess game.
#
#   curl -fsSL https://chess.thesudhanshu.dev/install.sh | sh
#
# Picks the build for this OS and CPU (Linux x86_64 or aarch64, macOS x86_64
# or arm64), checks the release signature and the archive's SHA-256, and puts
# the binary in ~/.local/bin. No sudo.
#
# Settings (environment variables):
#   CHESS_VERSION       a release tag such as v0.1.0 (default: the latest)
#   CHESS_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
#   CHESS_DOWNLOAD_URL  a folder holding the release files, instead of
#                       GitHub Releases (a mirror, or file:///... for testing)
#
# The signature is checked with `ssh-keygen -Y verify` (OpenSSH 8.1 or newer),
# which macOS and most Linux systems already have.

set -eu

REPO="https://github.com/sudhanshu19102003/chess/releases"

# The release signing key. Only SHA256SUMS files signed by it are accepted.
SIGNER="chess-release"
NAMESPACE="chess-release@sudhanshu19102003"
PUBKEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKHPBVfEIOY+rZ+qgRHy5c2S3YZ/drAl0Nrg9DWQxeSh"

say() { printf 'chess: %s\n' "$*"; }
die() { printf 'chess: error: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64 | amd64) echo x86_64-unknown-linux-musl ;;
                aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
                *) return 1 ;;
            esac
            ;;
        Darwin)
            # A shell running under Rosetta reports x86_64 on Apple silicon.
            if [ "$arch" = x86_64 ] &&
                [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
                arch=arm64
            fi
            case "$arch" in
                x86_64) echo x86_64-apple-darwin ;;
                arm64 | aarch64) echo aarch64-apple-darwin ;;
                *) return 1 ;;
            esac
            ;;
        *) return 1 ;;
    esac
}

fetch() { # URL FILE [progress]
    # Some networks silently drop one of GitHub's download addresses. A short
    # connect timeout makes curl move on to the next address after a few
    # seconds, instead of hanging for a minute or more.
    if has curl; then
        bar=-s
        [ -n "${3:-}" ] && [ -t 2 ] && bar=-#
        curl -fSL "$bar" --proto '=https,file' --tlsv1.2 --connect-timeout 8 \
            --retry 3 --retry-delay 1 -o "$2" "$1" || die "could not download $1"
    elif has wget; then
        wget -q --https-only --connect-timeout=8 --tries=3 -O "$2" "$1" ||
            die "could not download $1"
    else
        die "need curl or wget to download"
    fi
}

sha256() {
    if has sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    elif has openssl; then
        openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1
    else
        die "need sha256sum, shasum or openssl to check the download"
    fi
}

main() {
    target=$(target) || die "no build for $(uname -s) $(uname -m); build from source instead:
  https://github.com/sudhanshu19102003/chess#install"
    has ssh-keygen || die "need ssh-keygen (OpenSSH) to check the signature; install it
  (Debian/Ubuntu: apt install openssh-client, Alpine: apk add openssh-keygen)"
    has tar || die "need tar"

    version=${CHESS_VERSION:-}
    if [ -n "${CHESS_DOWNLOAD_URL:-}" ]; then
        base=${CHESS_DOWNLOAD_URL%/}
    elif [ -n "$version" ]; then
        base="$REPO/download/$version"
    else
        base="$REPO/latest/download"
    fi
    dir=${CHESS_INSTALL_DIR:-$HOME/.local/bin}
    archive="chess-$target.tar.gz"

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM

    say "downloading ${version:-the latest release} for $target"
    fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
    fetch "$base/SHA256SUMS.sig" "$tmp/SHA256SUMS.sig"

    printf '%s %s\n' "$SIGNER" "$PUBKEY" >"$tmp/allowed_signers"
    ssh-keygen -Y verify -f "$tmp/allowed_signers" -I "$SIGNER" -n "$NAMESPACE" \
        -s "$tmp/SHA256SUMS.sig" <"$tmp/SHA256SUMS" >/dev/null 2>&1 ||
        die "SHA256SUMS has no valid signature from the release key; not installing"

    want=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$want" ] || die "this release has no $archive"

    fetch "$base/$archive" "$tmp/$archive" progress
    [ "$(sha256 "$tmp/$archive")" = "$want" ] ||
        die "$archive does not match its signed checksum; not installing"
    say "signature and checksum OK"

    mkdir "$tmp/x"
    tar -xzf "$tmp/$archive" -C "$tmp/x"
    bin="$tmp/x/chess-$target/chess-tui"
    [ -f "$bin" ] || die "$archive has no chess-tui"

    mkdir -p "$dir"
    chmod 755 "$bin"
    # Move into place in one step, so a running copy is never half-written.
    cp "$bin" "$dir/.chess-tui.new"
    mv -f "$dir/.chess-tui.new" "$dir/chess-tui"
    say "installed $dir/chess-tui"

    case ":$PATH:" in
        *":$dir:"*) say "run it with: chess-tui" ;;
        *)
            say "$dir is not on your PATH. Add this line to your shell's startup file"
            say "(~/.zshrc, ~/.bashrc or ~/.profile), then open a new terminal:"
            printf '\n    export PATH="%s:$PATH"\n\n' "$dir"
            say "or run it now with: $dir/chess-tui"
            ;;
    esac
}

# Everything runs from here, so a download cut short runs nothing.
main "$@"
