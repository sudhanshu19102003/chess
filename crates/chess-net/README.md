# chess-net: hole punching and relay (milestones 3 and 4)

Measures how often two players on different networks get a direct TCP
connection, and falls back to a relay when they don't. Getting connected is
plain text lines, but the server never hands anyone the other player's address:
it pairs by nameplate and forwards opaque messages, and the players exchange
their addresses sealed under a key from the game code (`src/introduce.rs`),
so a stranger who guesses a nameplate learns nothing. Then the handshake (milestone 5) runs over the connection and
everything after it is encrypted. `chess-punch` ends by printing your colour,
the game id and one message from the other side, sent encrypted.

The game itself (`chess-tui`) uses the same two servers; the root
[README](../../README.md#playing-online) shows how to point it at them.

## On one machine

```sh
cargo run --release -p chess-net --bin chess-rendezvous
cargo run --release -p chess-net --bin chess-relay -- --port 4443
cargo run --release -p chess-net --bin chess-punch -- --relay 127.0.0.1:4443 host
cargo run --release -p chess-net --bin chess-punch -- --relay 127.0.0.1:4443 join CODE
```

Add `--force-relay` to both clients to test the relay path; on one machine
punching always works.

## Field test

1. **Server.** A machine with a public IPv4 address (a small VPS), TCP ports
   7777, 7778 and 443 open in its firewall:

   ```sh
   cargo build --release -p chess-net        # on the server (Linux)
   ./target/release/chess-rendezvous --log results.log
   sudo setcap cap_net_bind_service=+ep target/release/chess-relay
   ./target/release/chess-relay
   ```

2. **Two players on different networks**, e.g. one laptop on home broadband
   and another tethered to a phone hotspot (not on the same Wi-Fi):

   ```sh
   cargo run --release -p chess-net --bin chess-punch -- --server SERVER host
   # prints a code like 7-comet-pilot; on the other machine:
   cargo run --release -p chess-net --bin chess-punch -- --server SERVER join 7-comet-pilot
   ```

   Each run prints `CONNECTED via ...` (a direct path, or `relay` if
   punching failed for 20 s) or `NO CONNECTION`. On macOS, allow incoming connections if the firewall
   asks: the client listens on its port.

3. **Repeat** with different pairs of networks (home, office, mobile data,
   café Wi-Fi), both directions as host. Then:

   ```sh
   ./target/release/chess-rendezvous stats results.log
   ```

   gives the direct rate, broken down by each side's NAT kind (`eim`: one
   public port for every destination, punchable; `edm`: a new port per
   destination, "symmetric", usually not) and by the path that won.

## What the log records

One line per pairing: outcome (`direct`, `relay`, `failed`, `split` when the
sides disagree, `incomplete` when a client left), whether both had the same public
IP, and per side whether the NAT kept the local port (`kept`), the NAT kind,
the winning path (`public`/`private` address of the peer, `out` if this
side's connect won, `in` if its listener accepted), time to connect (`ms`)
and round trip (`rtt`). No IP addresses are written.

## Limits

- IPv4 only. Many mobile networks give IPv6, where direct connections only
  need to get past firewalls; worth a later look if the IPv4 rate is poor.
- `SO_REUSEPORT` is set on Unix only.
