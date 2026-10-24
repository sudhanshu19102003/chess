//! The rendezvous server for the hole-punch spike, and a summary of its log.

use std::path::PathBuf;
use std::process::ExitCode;

use chess_net::{DEFAULT_PORT, Stats, serve};
use tokio::net::TcpListener;

const USAGE: &str = "\
usage: chess-rendezvous [--port PORT] [--log FILE]
       chess-rendezvous stats FILE

Listens on PORT (default 7777) and PORT+1 (the NAT probe), both TCP, IPv4.
Each pairing's result is printed and appended to FILE.";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut port = DEFAULT_PORT;
    let mut log = None;
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["stats", file] => {
            return match std::fs::read_to_string(file) {
                Ok(text) => {
                    print!("{}", Stats::from_log(&text));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{file}: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        ref rest => {
            let mut rest = rest.iter();
            while let Some(flag) = rest.next() {
                match (*flag, rest.next()) {
                    ("--port", Some(p)) if p.parse::<u16>().is_ok_and(|p| p < u16::MAX) => {
                        port = p.parse().unwrap()
                    }
                    ("--log", Some(file)) => log = Some(PathBuf::from(file)),
                    _ => {
                        eprintln!("{USAGE}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
    }

    let bind = |port: u16| TcpListener::bind(("0.0.0.0", port));
    let (main, probe) = match (bind(port).await, bind(port + 1).await) {
        (Ok(main), Ok(probe)) => (main, probe),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("cannot listen on ports {port} and {}: {e}", port + 1);
            return ExitCode::FAILURE;
        }
    };
    eprintln!("rendezvous on port {port}, NAT probe on port {}", port + 1);
    match serve(main, probe, log).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
