# Terminal Chess

Two-player chess in the terminal, peer to peer. Both players' programs check
every move themselves; a small server only introduces them and never sees the
game. Written in Rust, with pixel-art pieces, chess clocks and a retro look.

![A game in progress](docs/screenshot.png)

> **Status:** a hobby project, not released yet. Offline play is complete.
> Online play is built and tested on one machine, but has not yet been played
> between two machines on different networks, and there is no public server
> yet (see [Playing online](#playing-online)).

## Contents

- [Install](#install)
- [Play](#play)
- [Playing online](#playing-online)
- [Fair play online](#fair-play-online)
- [How it works](#how-it-works)
- [Development](#development)
- [Credits](#credits)

## Install

### Today: build from source

You need [Rust](https://rustup.rs) (stable) on macOS or Linux. Windows is
untested. The repository is private for now, so cloning needs access to it.

```sh
git clone https://github.com/sudhanshu19102003/chess.git
cd chess
cargo install --path crates/chess-tui     # installs `chess-tui` into ~/.cargo/bin
chess-tui
```

To run your own servers for online play, also install them:

```sh
cargo install --path crates/chess-net     # chess-rendezvous, chess-relay, chess-punch
```

Or run without installing: `cargo run --release -p chess-tui`.

### Planned: one command

The goal is a single command that installs a signed binary:

```sh
curl -fsSL https://<domain>/install.sh | sh
```

This is the next milestone and isn't built yet. The plan:

1. **Release builds.** Tagging a release makes CI build a binary per
   platform: Linux x86_64 and aarch64 (static, with musl, so it runs on any
   distribution) and macOS x86_64 and aarch64. Windows comes later.
2. **Signing.** CI signs every archive and a `SHA256SUMS` file with an
   ed25519 key. The public key is written into `install.sh`, which refuses
   anything whose signature does not check out. The script itself is
   protected by HTTPS; the signature protects against a tampered download.
3. **`install.sh`.** Detects the OS and CPU with `uname`, downloads the
   matching archive, checks its signature, installs to `~/.local/bin` (no
   sudo) and says so if that folder is not on your `PATH`.
4. **Public servers.** A rendezvous server and relay on a public host, built
   in as the default, so online play works with no setup.
5. **Later, maybe:** a Homebrew tap, and `cargo install` from crates.io.

The code lives on GitHub (private for now), so releases would go on GitHub
Releases once it is public. Still to decide: the domain and host for the
servers, and the first platforms.

## Play

Make the terminal window about **146 × 50 cells**. The game is drawn at one
size; if the window is too small it asks you to zoom out, and if it is much
bigger it asks you to zoom in (Enter plays anyway).

The home screen offers **OFFLINE** (two players at one keyboard), **PEER TO
PEER**, and **VS COMPUTER** (not built yet). Pick a time control, press Enter,
and the game starts after a 3-2-1 countdown.

| Keys | |
|---|---|
| arrows or `hjkl` | move the cursor |
| Enter or Space | pick up a piece, drop it, confirm a promotion |
| Esc | let go, cancel |
| `Nf3`, `e4`, `O-O`… | type a move (start with a capital letter or `O`) |
| mouse | click or drag pieces, click buttons |
| `d` | offer a draw, or accept the other player's |
| `x` twice | resign |
| `f` | flip the board |
| `g` | high-resolution pieces on or off |
| `p` | pause the clock (offline only) |
| `r` | back to the home screen (twice in an unfinished online game) |
| `q` | quit |

All the rules are in: castling, en passant, promotion, check, checkmate,
stalemate, threefold repetition, the 50-move rule and insufficient material.
Repetition and the 50-move rule end the game automatically.

**Pieces and colours.** In terminals that draw Unicode 16 "octant"
characters, such as Ghostty, the pieces are drawn at full resolution. Elsewhere
they are drawn smaller, by hand, in the same palette. `CHESS_PIXELS=octant|half`
overrides the detection, and `g` switches in game. Colours are 24-bit when the
terminal says it supports them (`COLORTERM=truecolor`), otherwise 256-colour;
`CHESS_COLORS=256|truecolor` overrides.

## Playing online

An online game needs two small servers:

- **`chess-rendezvous`** pairs the two players by the number at the start of
  the game code. It never sees the rest of the code, and it never sees a move.
- **`chess-relay`** passes encrypted bytes between the players when they can't
  connect directly. It cannot read or change them.

There is no public server yet, and `chess-tui` looks for both on `127.0.0.1`
unless told otherwise.

### Try it on one machine

```sh
cargo run --release -p chess-net --bin chess-rendezvous          # terminal 1
cargo run --release -p chess-net --bin chess-relay -- --port 4443 # terminal 2
cargo run --release -p chess-tui -- --relay 127.0.0.1:4443       # terminal 3: host
cargo run --release -p chess-tui -- --relay 127.0.0.1:4443       # terminal 4: join
```

### Play between two machines

1. **Run the servers** on a machine with a public IPv4 address, such as a
   small VPS. Open TCP ports 7777, 7778 (the NAT probe) and 443 in its
   firewall.

   ```sh
   cargo build --release -p chess-net
   ./target/release/chess-rendezvous --log results.log
   sudo setcap cap_net_bind_service=+ep target/release/chess-relay   # Linux: lets it use port 443
   ./target/release/chess-relay
   ```

2. **Point both players at it**, with a flag or an environment variable:

   ```sh
   chess-tui --server your.server.example
   # or
   export CHESS_RENDEZVOUS=your.server.example
   ```

   The relay defaults to the same host on port 443. Use `--relay HOST[:PORT]`
   or `CHESS_RELAY` if it lives elsewhere.

3. **Host:** choose PEER TO PEER → HOST A GAME and a time control. You get a
   code like `7-comet-pilot`, already copied to your clipboard (`c` copies it
   again). Send it to your friend.

4. **Join:** choose PEER TO PEER → JOIN WITH A CODE and type the code.

5. The programs try to connect directly for up to 20 seconds, then fall back
   to the relay. Colours are decided by a fair coin flip that neither side can
   steer. On macOS, allow incoming connections if the firewall asks.

If the direct connection keeps failing, `--force-relay` on both sides skips
it.

## Fair play online

Neither player's program trusts the other, and there is no referee.

- **Every move is checked** by both programs: it must be legal, signed by the
  right player, and lead to the same position on both sides.
- **The clocks are agreed.** Each move carries the time the player says they
  spent on it. Both programs work out the clocks from these figures, so time
  lost on the network never counts against anyone.
- **Lag is checked.** Your program measures how long each of the opponent's
  moves really took to arrive. Some extra time is allowed for network delay,
  as a lag allowance that builds up with each move (the same idea Lichess
  uses). At 10 minutes it starts at 6 s and is capped at 14 s.
- **The game is drawn if the other player's program misbehaves:** the first
  message that fails a check, or a move that arrives later than its stated
  time by more than the lag allowance left. Your program hangs up and goes back
  to the home screen, which says why. A move that ends the game on the board
  keeps its result.
- **Running out of time:** an honest program concedes on its own when its
  clock runs out. If the opponent goes silent past their time and their lag
  allowance, you win on time on your screen.
- **No result:** leaving with `r` twice, losing the connection, or the two
  programs disagreeing about the position (a bug) ends the game with no
  result.

**Limits.** Without a referee, some things can't be prevented: engine
assistance, a player shaving a little off their stated thinking time (up to
the lag allowance), and a losing player quitting or sending a bad message to
end the game early. A finished game's signed move log can be checked by
anyone, but timing and disconnects are only seen by the two players.

## How it works

The workspace has four crates:

| Crate | What it does |
|---|---|
| `crates/chess-core` | The rules: board, legal moves, FEN, SAN, game endings. Tested with perft counts and against [`shakmaty`](https://crates.io/crates/shakmaty) at every move of random games. |
| `crates/chess-verify` | The move protocol: a strict binary message format, signatures (ed25519), the SHA-256 hash chain, the checks every message passes, and the agreed clock. |
| `crates/chess-net` | Getting connected: the rendezvous server, TCP hole punching, the relay, and the handshake (SPAKE2 from the game code, then a Noise session). |
| `crates/chess-tui` | The game you see: the home screen, lobby, board, clocks and the network task. |

How a game is set up:

1. The host asks the rendezvous server for a code. The server only knows the
   number; the two words are a password only the players know.
2. Both programs connect to each other directly if they can, or through the
   relay.
3. They prove they know the same code with SPAKE2, then open an encrypted
   Noise session keyed from it. A server that tried to sit in the middle
   would fail this step.
4. Each side makes a fresh signing key, they agree the time control, and they
   flip a coin for colours (each commits to a random number before revealing
   it).
5. Every move after that is signed and chained to the previous one, and both
   programs check it before playing it.

Both programs must run the same rules engine version; they refuse to play
otherwise.

## Development

```sh
cargo test -p chess-core -p chess-verify -p chess-net
cargo test -p chess-tui --lib --bins --tests          # examples/queens.rs is broken; skip it
cargo test --release -p chess-core -- --ignored       # deep perft runs
cargo clippy --workspace --lib --bins --tests
```

To see the screen without a terminal, render a PNG (uses macOS fonts):

```sh
cargo run --release --example snapshot -- 146 50 out.png midgame
```

Scenes include `home`, `countdown`, `start`, `midgame`, `captures`, `check`,
`mate`, `promotion`, `lobby`, `code`, `connecting`, `failed`, `online`,
`dropped` and `drawn`. Set `CHESS_PIXELS=octant` for high-resolution pieces.

`crates/chess-verify/fuzz` holds a fuzzer for the message parser (nightly
Rust and `cargo install cargo-fuzz`). `crates/chess-net/README.md` describes
the field test for measuring how often direct connections succeed.

Bump `chess_core::ENGINE_VERSION` with any change to legality, game endings,
FEN or SAN.

## Credits

A big shout-out to the [**Pixel Art Chess Set**](https://hollowsmilegames.itch.io/pixel-art-chess-set)
on itch.io ("Chess Asset 16bit pixel art" by Graciano Choc, gchoc). Every
piece on the board comes from it, and it is generously released to the
public domain (CC0). The sprite sheet and its notes are in
[`assets/pieces/`](assets/pieces/).

The lag allowance follows the design of [Lichess](https://lichess.org)'s open
source lag compensation in [scalachess](https://github.com/lichess-org/scalachess).
Game codes follow the idea behind [Magic Wormhole](https://github.com/magic-wormhole/magic-wormhole).
Built with [ratatui](https://ratatui.rs), [tokio](https://tokio.rs),
[snow](https://github.com/mcginty/snow), [spake2](https://crates.io/crates/spake2)
and [ed25519-dalek](https://crates.io/crates/ed25519-dalek).

## License

No license has been chosen yet, so all rights are reserved for now. The piece
art in `assets/pieces/` is CC0 and free for anyone to use.
