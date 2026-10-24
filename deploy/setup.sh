#!/bin/sh
# Installs or upgrades the public servers on a Linux machine with systemd:
# chess-rendezvous (TCP 7777, and 7778 for the NAT probe) and, only with
# --relay, chess-relay (TCP 443), each as a service that starts on boot and
# restarts on failure.
#
#   sudo sh setup.sh [--relay] chess-x86_64-unknown-linux-musl.tar.gz
#
# The archive is a release archive (scripts/package.sh). Running it again with
# a newer archive upgrades and restarts the servers. Without --relay, a relay
# set up by an earlier run is stopped and disabled, so games that cannot
# connect directly fail instead of falling back to it. deploy/deploy.sh copies
# the files to a Google Cloud VM and runs this there.
#
# Both run as a throwaway unprivileged user (systemd DynamicUser); the relay
# gets only the right to bind port 443. The rendezvous log of connection
# results is /var/lib/chess/results.log:
#
#   sudo chess-rendezvous stats /var/lib/chess/results.log
#
# The firewall must let TCP 7777 and 7778 in, and 443 for the relay (on Google
# Cloud, a VPC firewall rule; this script does not touch it).

set -eu

die() { printf 'setup: error: %s\n' "$*" >&2; exit 1; }

relay=no
if [ "${1:-}" = --relay ]; then
    relay=yes
    shift
fi
archive=${1:?usage: sudo sh setup.sh [--relay] chess-<target>.tar.gz}
[ "$(id -u)" = 0 ] || die "run as root (sudo)"
[ -d /run/systemd/system ] || die "needs systemd"
[ -f "$archive" ] || die "no such file: $archive"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
tar -xzf "$archive" -C "$tmp"
for bin in chess-rendezvous chess-relay; do
    src=$(find "$tmp" -type f -name "$bin" | head -n 1)
    [ -n "$src" ] || die "$archive has no $bin"
    install -m 755 "$src" "/usr/local/bin/$bin.new"
    mv -f "/usr/local/bin/$bin.new" "/usr/local/bin/$bin"
done

# Settings both services share: no privileges, read-only system.
common='DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
Restart=always
RestartSec=2'

cat >/etc/systemd/system/chess-rendezvous.service <<EOF
[Unit]
Description=chess rendezvous server (pairs players by game code)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/chess-rendezvous --log /var/lib/chess/results.log
StateDirectory=chess
CapabilityBoundingSet=
$common

[Install]
WantedBy=multi-user.target
EOF

[ "$relay" = yes ] && cat >/etc/systemd/system/chess-relay.service <<EOF
[Unit]
Description=chess relay (forwards encrypted games that cannot connect directly)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/chess-relay
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
# Up to 1000 sessions, two sockets each.
LimitNOFILE=4096
$common

[Install]
WantedBy=multi-user.target
EOF

units=chess-rendezvous
[ "$relay" = yes ] && units="$units chess-relay"

systemctl daemon-reload
for unit in $units; do
    systemctl enable --quiet "$unit"
    systemctl restart "$unit"
done
if [ "$relay" = no ] && { systemctl is-enabled --quiet chess-relay 2>/dev/null ||
    systemctl is-active --quiet chess-relay 2>/dev/null; }; then
    systemctl disable --now --quiet chess-relay
    echo "setup: chess-relay stopped and disabled (pass --relay to run it)"
fi

sleep 1
failed=0
for unit in $units; do
    if systemctl is-active --quiet "$unit"; then
        echo "setup: $unit running"
    else
        echo "setup: $unit failed to start:" >&2
        journalctl -u "$unit" -n 20 --no-pager >&2 || true
        failed=1
    fi
done
exit "$failed"
