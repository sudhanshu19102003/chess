//! Games on the local network, on this machine: the host and the joiner
//! find each other by the loopback copy of their broadcasts. Each test has
//! its own UDP port, so tests running at once do not hear each other.

use std::time::Duration;

use chess_net::connect::{Error, Stage};
use chess_net::handshake::{self, Offer, TimeControl};
use chess_net::{Via, lan};
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
const LIMIT: Duration = Duration::from_secs(20);

/// Hosts on `port` and joins with the host's code, or with `typed` (where
/// `NAMEPLATE` stands for the host's nameplate).
async fn pair(
    port: u16,
    typed: Option<&str>,
) -> (Result<chess_net::connect::Linked, Error>, Result<chess_net::connect::Linked, Error>, Vec<Stage>) {
    let (code_tx, mut code_rx) = mpsc::unbounded_channel();
    let host = lan::host(OFFER, port, |stage| {
        if let Stage::Code(code) = stage {
            let _ = code_tx.send(code);
        }
    });
    let join = async {
        let code: String = code_rx.recv().await.unwrap();
        let code = match typed {
            Some(typed) => typed.replace("NAMEPLATE", code.split('-').nth(1).unwrap()),
            None => code,
        };
        let mut stages = Vec::new();
        let linked = lan::join(&code, port, |stage| stages.push(stage)).await;
        (linked, stages)
    };
    let (host, (join, stages)) = timeout(LIMIT, async { tokio::join!(host, join) }).await.unwrap();
    (host, join, stages)
}

#[tokio::test]
async fn host_and_joiner_find_each_other() {
    let (host, join, stages) = pair(47791, None).await;
    let (host, join) = (host.unwrap(), join.unwrap());
    assert_eq!(host.established.game_id, join.established.game_id);
    assert_eq!(host.established.me, join.established.me.opponent());
    assert_eq!(join.established.time_control, TC);
    assert_eq!((host.path.via, join.path.via), (Via::Lan, Via::Lan));
    assert_eq!(stages, [Stage::Searching, Stage::Handshake]);
}

#[tokio::test]
async fn a_wrong_code_fails_on_both_sides() {
    // The right nameplate finds the host, and the handshake catches the
    // wrong words.
    let (host, join, _) = pair(47792, Some("lan-NAMEPLATE-acid-acorn")).await;
    for result in [host, join] {
        assert!(matches!(result, Err(Error::Handshake(handshake::Error::WrongCode))));
    }
}

#[tokio::test]
async fn codes_are_checked_before_looking() {
    for typed in ["hello", "7-comet-pilot", "lan-comet-pilot"] {
        assert!(matches!(lan::join(typed, 47793, |_| {}).await, Err(Error::BadCode)));
    }
}

#[tokio::test(start_paused = true)]
async fn the_joiner_gives_up_when_nobody_hosts() {
    let result = lan::join("lan-7-comet-pilot", 47794, |_| {}).await;
    assert!(matches!(result, Err(Error::NoConnection(e)) if e.kind() == std::io::ErrorKind::TimedOut));
}
