//! The relay server: pairs two players by session token and forwards bytes.

use std::process::ExitCode;

use chess_net::relay::DEFAULT_RELAY_PORT;
use chess_net::{Limits, serve_relay};
use tokio::net::TcpListener;

const USAGE: &str = "\
usage: chess-relay [--port PORT]

Listens on PORT (default 443), TCP, IPv4. On Linux a port below 1024 needs
root or: sudo setcap cap_net_bind_service=+ep chess-relay";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => DEFAULT_RELAY_PORT,
        ["--port", port] if port.parse::<u16>().is_ok() => port.parse().unwrap(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("cannot listen on port {port}: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("relay on port {port}");
    match serve_relay(listener, Limits::default()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
