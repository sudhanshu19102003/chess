//! The rendezvous server, relay and clients end to end on one machine, over
//! loopback. This exercises the protocols and the shared-port sockets, not a
//! real NAT; that takes two networks (see CLAUDE.md, milestone 3).

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use chess_net::code::{WORDS, new_code};
use chess_net::{
    Limits, Nat, Peer, Rendezvous, Report, Role, Stats, Via, measure_rtt, serve, serve_relay,
    via_relay,
};
use tokio::net::TcpListener;

async fn start_server(log: Option<std::path::PathBuf>) -> (SocketAddr, SocketAddr) {
    let main = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addrs = (main.local_addr().unwrap(), probe.local_addr().unwrap());
    tokio::spawn(serve(main, probe, log));
    addrs
}

/// Connects directly, or through `relay` if given, and reports.
async fn play(
    mut rendezvous: Rendezvous,
    role: Role,
    peer: Peer,
    relay: Option<SocketAddr>,
) -> io::Result<(Peer, Report)> {
    let nat = rendezvous.nat(&peer);
    let limit = Duration::from_secs(10);
    let mut connection = match relay {
        None => rendezvous.punch(&peer, role, limit).await?,
        Some(relay) => via_relay(relay, &peer, role, limit).await?,
    };
    let rtt = measure_rtt(&mut connection.stream, role).await?;
    let report = Report::Connected {
        nat,
        path: connection.path,
        elapsed: connection.elapsed,
        rtt,
    };
    rendezvous.report(&report).await?;
    Ok((peer, report))
}

/// Pairs two clients, connects them directly or through `relay`, and
/// returns each side's introduction, report and address, and the stats of
/// the server's log (kept in a file named after `name`).
async fn pair_logged(
    relay: Option<SocketAddr>,
    name: &str,
) -> ([(Peer, Report, SocketAddr); 2], Stats) {
    let log = std::env::temp_dir().join(format!(
        "chess-net-test-{}-{}.log",
        std::process::id(),
        name.replace(|c: char| !c.is_ascii_alphanumeric(), "")
    ));
    let _ = std::fs::remove_file(&log);
    let (server, probe) = start_server(Some(log.clone())).await;

    let mut host = Rendezvous::connect(server, probe).await.unwrap();
    let nameplate = host.host().await.unwrap();
    assert_eq!(nameplate, 1);
    let code = new_code(nameplate);
    let mut joiner = Rendezvous::connect(server, probe).await.unwrap();
    let (host_addr, join_addr) = (host.private_addr(), joiner.private_addr());

    let host_side = async {
        let peer = host.wait_peer(&code).await?;
        play(host, Role::Host, peer, relay).await
    };
    let join_side = async {
        let peer = joiner.join(nameplate, &code).await?;
        play(joiner, Role::Join, peer, relay).await
    };
    let (host_result, join_result) = tokio::join!(host_side, join_side);
    let (host_peer, host_report) = host_result.unwrap();
    let (join_peer, join_report) = join_result.unwrap();
    let stats = Stats::from_log(&std::fs::read_to_string(&log).unwrap());
    let _ = std::fs::remove_file(&log);
    (
        [
            (host_peer, host_report, host_addr),
            (join_peer, join_report, join_addr),
        ],
        stats,
    )
}

#[tokio::test]
async fn two_clients_connect_directly_and_the_server_logs_it() {
    let (
        [
            (host_peer, host_report, host_addr),
            (join_peer, join_report, join_addr),
        ],
        stats,
    ) = pair_logged(None, "direct").await;
    assert_eq!(host_peer.token, join_peer.token);
    assert_eq!(host_peer.public, join_addr);
    assert_eq!(join_peer.public, host_addr);
    for report in [&host_report, &join_report] {
        let Report::Connected { nat, path, .. } = report else {
            panic!()
        };
        assert_eq!(*nat, Nat::None);
        assert_eq!(path.via, Via::Public);
    }
    assert_eq!((stats.direct, stats.reported()), (1, 1));
    assert_eq!(stats.by_nat["none+none"], (1, 1));
}

#[tokio::test]
async fn two_clients_connect_through_the_relay_and_the_server_logs_it() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = listener.local_addr().unwrap();
    tokio::spawn(serve_relay(listener, Limits::default()));
    let ([(_, host_report, _), (_, join_report, _)], stats) =
        pair_logged(Some(relay), "relay").await;
    for report in [&host_report, &join_report] {
        let Report::Connected { path, .. } = report else {
            panic!()
        };
        assert_eq!(path.via, Via::Relay);
        assert!(report.to_string().starts_with("RESULT link=relay "));
    }
    assert_eq!((stats.relay, stats.direct, stats.reported()), (1, 0, 1));
}

#[tokio::test]
async fn a_relay_nobody_listens_on_fails_cleanly() {
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let peer = Peer {
        token: "00".into(),
        you: closed,
        public: closed,
        private: closed,
    };
    let result = via_relay(closed, &peer, Role::Host, Duration::from_secs(1)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn unknown_code_is_refused() {
    let (server, probe) = start_server(None).await;
    let mut joiner = Rendezvous::connect(server, probe).await.unwrap();
    let err = joiner.join(42, "42-comet-pilot").await.unwrap_err();
    assert!(err.to_string().contains("unknown code"), "{err}");
}

#[tokio::test]
async fn codes_are_single_use() {
    let (server, probe) = start_server(None).await;
    let mut host = Rendezvous::connect(server, probe).await.unwrap();
    let nameplate = host.host().await.unwrap();
    let code = new_code(nameplate);
    let mut first = Rendezvous::connect(server, probe).await.unwrap();
    let (waiting, joined) = tokio::join!(host.wait_peer(&code), first.join(nameplate, &code));
    waiting.unwrap();
    joined.unwrap();
    let mut second = Rendezvous::connect(server, probe).await.unwrap();
    assert!(second.join(nameplate, &code).await.is_err());
}

/// The server forwards opaque messages and never an address: a stranger who
/// knows only the nameplate, even one who gets paired, learns nothing, and
/// the host is told the code did not match.
#[tokio::test]
async fn a_joiner_without_the_words_learns_no_address() {
    let (server, probe) = start_server(None).await;
    let mut host = Rendezvous::connect(server, probe).await.unwrap();
    let nameplate = host.host().await.unwrap();
    let code = new_code(nameplate);
    let mut stranger = Rendezvous::connect(server, probe).await.unwrap();
    // Whatever the stranger guesses at the words, it is not the code.
    let guess = format!("{nameplate}-{}-{}", WORDS[0], WORDS[0]);
    assert_ne!(guess, code);
    let (waiting, joined) = tokio::join!(host.wait_peer(&code), stranger.join(nameplate, &guess));
    for result in [waiting, joined] {
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }
}

/// What a stranger's own connection sees, line by line: its own address,
/// and nothing about the host.
#[tokio::test]
async fn the_server_sends_a_joiner_no_address_of_the_host() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (server, probe) = start_server(None).await;
    let mut host = Rendezvous::connect(server, probe).await.unwrap();
    let nameplate = host.host().await.unwrap();
    let host_addr = host.private_addr();
    let stream = tokio::net::TcpStream::connect(server).await.unwrap();
    let mine = stream.local_addr().unwrap();
    let mut raw = BufReader::new(stream);
    raw.get_mut().write_all(format!("JOIN {nameplate} {mine}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    raw.read_line(&mut line).await.unwrap();
    assert_eq!(line, format!("PAIRED {mine}\n"));
    assert!(!line.contains(&host_addr.port().to_string()));
    // The host has said nothing, so the server has nothing more to send.
    line.clear();
    let more = tokio::time::timeout(Duration::from_millis(300), raw.read_line(&mut line)).await;
    assert!(more.is_err(), "{line}");
}

#[tokio::test]
async fn joins_from_one_address_are_limited() {
    let (server, probe) = start_server(None).await;
    let mut last = String::new();
    for n in 1..=12u16 {
        let mut joiner = Rendezvous::connect(server, probe).await.unwrap();
        last = joiner.join(n, &new_code(n)).await.unwrap_err().to_string();
    }
    assert!(last.contains("too many tries"), "{last}");
}

#[tokio::test]
async fn punch_gives_up_on_an_unreachable_peer() {
    let (server, probe) = start_server(None).await;
    let rendezvous = Rendezvous::connect(server, probe).await.unwrap();
    // A closed port: every attempt is refused.
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let peer = Peer {
        token: "00".into(),
        you: rendezvous.private_addr(),
        public: closed,
        private: closed,
    };
    let err = rendezvous
        .punch(&peer, Role::Host, Duration::from_secs(1))
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn a_connection_from_a_stranger_is_not_taken_for_the_peer() {
    let (server, probe) = start_server(None).await;
    let rendezvous = Rendezvous::connect(server, probe).await.unwrap();
    let stranger = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = stranger.local_addr().unwrap();
    // Accepts and answers with a wrong token.
    tokio::spawn(async move {
        loop {
            let (mut s, _) = stranger.accept().await.unwrap();
            let _ = tokio::io::AsyncWriteExt::write_all(&mut s, b"CHESS-PUNCH 1 ff join\n").await;
        }
    });
    let peer = Peer {
        token: "00".into(),
        you: rendezvous.private_addr(),
        public: addr,
        private: addr,
    };
    let err = rendezvous
        .punch(&peer, Role::Host, Duration::from_secs(1))
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
}

/// One address may keep only a few codes waiting, so it cannot fill the
/// server's slots.
#[tokio::test]
async fn one_address_cannot_hold_many_waiting_codes() {
    let (server, probe) = start_server(None).await;
    let mut hosts = Vec::new();
    for _ in 0..3 {
        let mut host = Rendezvous::connect(server, probe).await.unwrap();
        host.host().await.unwrap();
        hosts.push(host);
    }
    let mut fourth = Rendezvous::connect(server, probe).await.unwrap();
    let err = fourth.host().await.unwrap_err();
    assert!(err.to_string().contains("too many codes"), "{err}");
    // A slot comes back when a host leaves.
    hosts.pop();
    let mut again = Rendezvous::connect(server, probe).await.unwrap();
    // The server notices the closed connection a moment later.
    let mut result = again.host().await;
    for _ in 0..20 {
        if result.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        again = Rendezvous::connect(server, probe).await.unwrap();
        result = again.host().await;
    }
    result.unwrap();
}

#[tokio::test]
async fn one_address_cannot_hold_many_connections() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (server, _) = start_server(None).await;
    // Idle connections that never say what they want.
    let mut idle = Vec::new();
    for _ in 0..6 {
        idle.push(tokio::net::TcpStream::connect(server).await.unwrap());
    }
    let mut seventh = BufReader::new(tokio::net::TcpStream::connect(server).await.unwrap());
    let mut line = String::new();
    seventh.read_line(&mut line).await.unwrap();
    assert_eq!(line, "ERR too many connections\n");
}
