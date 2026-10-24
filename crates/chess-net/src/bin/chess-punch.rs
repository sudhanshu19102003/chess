//! The connection test client: meet the other player through the
//! rendezvous server, try a direct connection, fall back to the relay,
//! report how it went, then run the handshake and swap one encrypted
//! message.

use std::io;
use std::net::SocketAddr;
use std::process::ExitCode;

use chess_net::client::{PUNCH_LIMIT, RELAY_LIMIT};
use chess_net::code::{new_code, parse_code};
use chess_net::connect::resolve;
use chess_net::handshake::{self, Offer, TimeControl};
use chess_net::relay::DEFAULT_RELAY_PORT;
use chess_net::{DEFAULT_PORT, Nat, Rendezvous, Report, Role, measure_rtt, via_relay};

/// The host's offer in this test: five minutes, no increment.
const TIME_CONTROL: TimeControl = TimeControl {
    base_ms: 5 * 60 * 1000,
    increment_ms: 0,
};

const USAGE: &str = "\
usage: chess-punch [OPTIONS] host
       chess-punch [OPTIONS] join CODE

options:
  --server HOST[:PORT]  rendezvous server: $CHESS_RENDEZVOUS, or
                        play.chess.thesudhanshu.dev; port 7777
  --relay HOST[:PORT]   relay: the server's host, port 443
  --force-relay         skip the direct attempt (both sides must pass it)";

struct Options {
    server: String,
    relay: Option<String>,
    force_relay: bool,
    /// The nameplate and the whole code to join, or `None` to host.
    join: Option<(u16, String)>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1).collect()) {
        Ok(options) => options,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    match run(&options).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<Options, String> {
    let mut options = Options {
        server: std::env::var("CHESS_RENDEZVOUS")
            .unwrap_or_else(|_| chess_net::DEFAULT_SERVER.into()),
        relay: None,
        force_relay: false,
        join: None,
    };
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => options.server = args.next().ok_or(USAGE)?,
            "--relay" => options.relay = Some(args.next().ok_or(USAGE)?),
            "--force-relay" => options.force_relay = true,
            _ => rest.push(arg),
        }
    }
    match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["host"] => {}
        ["join", code] => match parse_code(code) {
            Some((nameplate, words)) => {
                options.join = Some((nameplate, format!("{nameplate}-{words}")))
            }
            None => {
                return Err(format!(
                    "not a game code: {code} (expected something like 7-comet-pilot)"
                ));
            }
        },
        _ => return Err(USAGE.into()),
    }
    Ok(options)
}

/// True if the players got a connection, direct or relayed, and finished
/// the handshake over it.
async fn run(options: &Options) -> io::Result<bool> {
    let server = resolve(&options.server, DEFAULT_PORT).await?;
    let relay = match &options.relay {
        Some(relay) => resolve(relay, DEFAULT_RELAY_PORT).await?,
        None => SocketAddr::new(server.ip(), DEFAULT_RELAY_PORT),
    };
    let probe = SocketAddr::new(server.ip(), server.port() + 1);
    println!("rendezvous server {server}, relay {relay}");
    let mut rendezvous = Rendezvous::connect(server, probe).await?;
    println!("this machine: {}", rendezvous.private_addr());

    let (role, peer, code) = match &options.join {
        None => {
            let code = new_code(rendezvous.host().await?);
            // As this program was started, e.g. target/release/chess-punch,
            // so the line can be pasted as it is.
            let program = std::env::args()
                .next()
                .unwrap_or_else(|| "chess-punch".into());
            println!("\ngame code: {code}");
            let flags = format!(
                "{}{}",
                if options.relay.is_some() {
                    format!(" --relay {relay}")
                } else {
                    String::new()
                },
                if options.force_relay {
                    " --force-relay"
                } else {
                    ""
                },
            );
            println!(
                "in another terminal, or on the other machine, run:\n  {program} --server {}{flags} join {code}\n",
                options.server
            );
            println!("waiting for the other player...");
            (Role::Host, rendezvous.wait_peer().await?, code)
        }
        Some((nameplate, code)) => (Role::Join, rendezvous.join(*nameplate).await?, code.clone()),
    };
    let nat = rendezvous.nat(&peer);
    let nat_text = match nat {
        Nat::None => "no NAT",
        Nat::Eim => "NAT keeps one public port for every destination, good for punching",
        Nat::Edm => "NAT uses a new public port per destination, hard to punch",
        Nat::Unknown => "NAT probe got no answer",
    };
    println!("server sees you at {} ({nat_text})", peer.you);
    println!("peer: public {}, private {}", peer.public, peer.private);

    let start = std::time::Instant::now();
    let direct = if options.force_relay {
        println!("skipping the direct attempt");
        None
    } else {
        println!("punching for up to {} s...", PUNCH_LIMIT.as_secs());
        match rendezvous.punch(&peer, role, PUNCH_LIMIT).await {
            Ok(connection) => Some(connection),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                println!("no direct connection");
                None
            }
            Err(e) => return Err(e),
        }
    };
    let connection = match direct {
        Some(connection) => Ok(connection),
        None => {
            println!("trying the relay for up to {} s...", RELAY_LIMIT.as_secs());
            via_relay(relay, &peer, role, RELAY_LIMIT).await
        }
    };

    let (report, connection) = match connection {
        Ok(mut connection) => {
            let rtt = measure_rtt(&mut connection.stream, role).await?;
            let elapsed = start.elapsed();
            println!(
                "CONNECTED via {} after {} ms, round trip {:.1} ms",
                connection.path,
                elapsed.as_millis(),
                rtt.as_secs_f64() * 1000.0
            );
            let report = Report::Connected {
                nat,
                path: connection.path,
                elapsed,
                rtt,
            };
            (report, Some(connection))
        }
        Err(e) => {
            println!("NO CONNECTION: {e}");
            let report = Report::Failed {
                nat,
                waited: start.elapsed(),
            };
            (report, None)
        }
    };
    rendezvous.report(&report).await?;
    println!("result recorded by the server");
    let Some(connection) = connection else {
        return Ok(false);
    };

    println!("handshake with code {code}...");
    let shaken = match role {
        Role::Host => {
            let offer = Offer {
                time_control: TIME_CONTROL,
                colour: None,
            };
            handshake::host(connection.stream, &code, offer).await
        }
        Role::Join => handshake::join(connection.stream, &code).await,
    };
    let mut game = match shaken {
        Ok(game) => game,
        Err(e) => {
            println!("HANDSHAKE FAILED: {e}");
            return Ok(false);
        }
    };
    let id: String = game.game_id.0.iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "SECURE SESSION: you play {:?}, {} min + {} s, game id {id}",
        game.me,
        game.time_control.base_ms / 60_000,
        game.time_control.increment_ms / 1000
    );
    game.channel
        .send(format!("greetings from the {role}").as_bytes())
        .await?;
    let greeting = game.channel.recv().await?;
    println!("encrypted message: {}", String::from_utf8_lossy(&greeting));
    Ok(true)
}
