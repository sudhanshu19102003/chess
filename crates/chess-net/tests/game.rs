//! What the game uses: `connect` from code to secure channel, and `link`
//! carrying messages afterwards, on loopback.

use std::net::SocketAddr;
use std::time::Duration;

use chess_net::connect::{self, Error, Servers, Stage, Start};
use chess_net::handshake::{self, Offer, TimeControl};
use chess_net::link::{self, Event};
use chess_net::{Limits, Via, serve, serve_relay};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

const TC: TimeControl = TimeControl {
    base_ms: 180_000,
    increment_ms: 2_000,
};
const OFFER: Offer = Offer {
    time_control: TC,
    colour: None,
};

/// A rendezvous server and a relay on loopback.
async fn servers(force_relay: bool) -> Servers {
    let main = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // The probe must be on the next port up, as the client assumes.
    let port = main.local_addr().unwrap().port();
    let Ok(probe) = TcpListener::bind(("127.0.0.1", port + 1)).await else {
        // Taken by something else; any failure to probe only makes the NAT
        // kind unknown, so a stray port is fine.
        return Box::pin(servers(force_relay)).await;
    };
    let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr: SocketAddr = relay.local_addr().unwrap();
    tokio::spawn(serve(main, probe, None));
    tokio::spawn(serve_relay(relay, Limits::default()));
    Servers {
        rendezvous: format!("127.0.0.1:{port}"),
        relay: Some(relay_addr.to_string()),
        force_relay,
    }
}

/// Hosts, joins with the host's code (or `typed`, if given), and returns
/// both results and the stages each side went through.
async fn pair(
    servers: &Servers,
    typed: Option<&str>,
) -> (
    Result<connect::Linked, Error>,
    Result<connect::Linked, Error>,
    Vec<Stage>,
    Vec<Stage>,
) {
    let (code_tx, mut code_rx) = mpsc::unbounded_channel();
    let mut host_stages = Vec::new();
    let mut join_stages = Vec::new();
    let host = connect::connect(servers, Start::Host(OFFER), |stage| {
        if let Stage::Code(code) = &stage {
            code_tx.send(code.clone()).unwrap();
        }
        host_stages.push(stage);
    });
    let join = async {
        let code = code_rx.recv().await.unwrap();
        let code = match typed {
            Some(typed) => typed.replace("NAMEPLATE", code.split('-').next().unwrap()),
            None => code,
        };
        connect::connect(servers, Start::Join(code), |stage| join_stages.push(stage)).await
    };
    let (host, join) = timeout(Duration::from_secs(60), async { tokio::join!(host, join) })
        .await
        .unwrap();
    (host, join, host_stages, join_stages)
}

#[tokio::test]
async fn host_and_joiner_connect_directly() {
    let servers = servers(false).await;
    let (host, join, host_stages, join_stages) = pair(&servers, None).await;
    let (host, join) = (host.unwrap(), join.unwrap());
    assert_eq!(host.established.game_id, join.established.game_id);
    assert_eq!(host.established.me, join.established.me.opponent());
    assert_eq!(join.established.time_control, TC);
    assert_ne!(host.path.via, Via::Relay);
    assert!(matches!(&host_stages[..], [Stage::Server, Stage::Code(_), Stage::Punching, Stage::Handshake]));
    assert_eq!(join_stages, [Stage::Server, Stage::Punching, Stage::Handshake]);
}

#[tokio::test]
async fn forced_relay() {
    let servers = servers(true).await;
    let (host, join, host_stages, _) = pair(&servers, None).await;
    let (host, join) = (host.unwrap(), join.unwrap());
    assert_eq!(host.path.via, Via::Relay);
    assert_eq!(host.established.game_id, join.established.game_id);
    assert!(matches!(&host_stages[..], [Stage::Server, Stage::Code(_), Stage::Relay, Stage::Handshake]));
}

#[tokio::test]
async fn a_wrong_code_fails_on_both_sides() {
    let servers = servers(false).await;
    // The right nameplate with the wrong words reaches the host, and the
    // handshake catches it.
    let (host, join, _, _) = pair(&servers, Some("NAMEPLATE-acid-acorn")).await;
    for result in [host, join] {
        assert!(matches!(result, Err(Error::Handshake(handshake::Error::WrongCode))));
    }
    // A code that is no code at all never leaves this machine.
    let bad = connect::connect(&servers, Start::Join("hello".into()), |_| {}).await;
    assert!(matches!(bad, Err(Error::BadCode)));
}

/// The next event that is not a round-trip estimate.
async fn next(rx: &mut mpsc::UnboundedReceiver<Event>) -> Event {
    loop {
        match timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap() {
            Event::Rtt(_) => continue,
            other => return other,
        }
    }
}

#[tokio::test]
async fn link_carries_messages_pings_and_the_end() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (host, join) = tokio::join!(handshake::host(a, "7-comet-pilot", OFFER), handshake::join(b, "7-comet-pilot"));
    let (host, join) = (host.unwrap(), join.unwrap());

    let (host_out, host_out_rx) = mpsc::unbounded_channel();
    let (host_events_tx, mut host_events) = mpsc::unbounded_channel();
    let (join_out, join_out_rx) = mpsc::unbounded_channel();
    let (join_events_tx, mut join_events) = mpsc::unbounded_channel();
    tokio::spawn(link::run(host.channel, host_out_rx, host_events_tx));
    tokio::spawn(link::run(join.channel, join_out_rx, join_events_tx));

    // Messages pass through as they are.
    host_out.send(vec![1, 2, 3]).unwrap();
    join_out.send(vec![0x10, 9]).unwrap();
    assert!(matches!(next(&mut join_events).await, Event::Message { bytes, .. } if bytes == [1, 2, 3]));
    // A Ping of the wrong length is not a Ping.
    assert!(matches!(next(&mut host_events).await, Event::Message { bytes, .. } if bytes == [0x10, 9]));

    // The pings give each side a round-trip estimate.
    let rtt = loop {
        if let Event::Rtt(rtt) = timeout(Duration::from_secs(10), host_events.recv()).await.unwrap().unwrap() {
            break rtt;
        }
    };
    assert!(rtt < Duration::from_secs(1));

    // Hanging up one side closes the other.
    drop(host_out);
    assert!(matches!(next(&mut join_events).await, Event::Closed(_)));
}
