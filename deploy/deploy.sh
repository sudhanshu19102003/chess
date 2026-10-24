#!/bin/sh
# Builds the Linux x86_64 archive and installs it on the Google Cloud VM that
# runs the public servers, with deploy/setup.sh. Run it from a machine with
# the gcloud CLI logged in (`gcloud auth login`, `gcloud config set project`).
#
#   deploy/deploy.sh [--relay] VM_NAME [ZONE]
#
# --relay also runs chess-relay (see deploy/setup.sh); without it the relay is
# off. ZONE defaults to gcloud's configured zone. Run it again to upgrade.

set -eu

relay=
if [ "${1:-}" = --relay ]; then
    relay="--relay "
    shift
fi
vm=${1:?usage: deploy/deploy.sh [--relay] VM_NAME [ZONE]}
zone=${2:-}
cd "$(dirname "$0")/.."
set --
[ -n "$zone" ] && set -- --zone "$zone"

target=x86_64-unknown-linux-musl
archive=$(scripts/package.sh "$target" | tail -n 1)

gcloud compute scp "$@" "$archive" deploy/setup.sh "$vm:"
gcloud compute ssh "$@" "$vm" --command "sudo sh setup.sh $relay$(basename "$archive")"
