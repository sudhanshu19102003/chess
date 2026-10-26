#!/bin/sh
# Builds a Linux archive and installs it on the machine that runs the public
# servers, with deploy/setup.sh, over ssh. Run it from a machine that can
# `ssh` to the server with a key (no gcloud or other cloud tools needed).
#
#   deploy/deploy.sh [--relay] [--target TRIPLE] USER@HOST [SSH_OPTION...]
#
# --relay also runs chess-relay (see deploy/setup.sh); without it the relay is
# off. TRIPLE is the server's architecture: aarch64-unknown-linux-musl (the
# default, e.g. Oracle Cloud's free Ampere A1 instance) or
# x86_64-unknown-linux-musl. SSH_OPTIONs go to both scp and ssh, e.g.
# `-i ~/.ssh/oracle.key` (use -o IdentityFile=... if scp does not take it).
# Run it again to upgrade.

set -eu

relay=
target=aarch64-unknown-linux-musl
while :; do
    case "${1:-}" in
        --relay) relay="--relay "; shift ;;
        --target) target=${2:?--target needs a value}; shift 2 ;;
        *) break ;;
    esac
done
host=${1:?usage: deploy/deploy.sh [--relay] [--target TRIPLE] USER@HOST [SSH_OPTION...]}
shift
cd "$(dirname "$0")/.."

archive=$(scripts/package.sh "$target" | tail -n 1)

scp "$@" "$archive" deploy/setup.sh "$host:"
ssh "$@" "$host" "sudo sh setup.sh $relay$(basename "$archive")"
