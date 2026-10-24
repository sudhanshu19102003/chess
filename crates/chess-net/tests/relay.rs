//! The relay on its own, driven with raw sockets.

use std::time::Duration;

use chess_net::{Limits, serve_relay};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

async fn start(limits: Limits) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_relay(listener, limits));
    addr
}

/// Connects, names `token`, and returns the stream and the first reply.
async fn open(relay: std::net::SocketAddr, token: &str) -> (BufReader<TcpStream>, String) {
    let mut conn = BufReader::new(TcpStream::connect(relay).await.unwrap());
    conn.get_mut()
        .write_all(format!("RELAY {token}\n").as_bytes())
        .await
        .unwrap();
    let mut reply = String::new();
    conn.read_line(&mut reply).await.unwrap();
    (conn, reply.trim_end().to_string())
}

async fn open_pair(relay: std::net::SocketAddr) -> (BufReader<TcpStream>, BufReader<TcpStream>) {
    let (a, b) = tokio::join!(open(relay, TOKEN), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        open(relay, TOKEN).await
    });
    assert_eq!((a.1.as_str(), b.1.as_str()), ("PAIRED", "PAIRED"));
    (a.0, b.0)
}

/// Reads to the end of the stream; true if it ended rather than timing out.
async fn closes(conn: &mut BufReader<TcpStream>) -> bool {
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), conn.read_to_end(&mut rest))
        .await
        .is_ok()
}

#[tokio::test]
async fn forwards_both_ways_and_passes_the_end_on() {
    let relay = start(Limits::default()).await;
    let (mut a, mut b) = open_pair(relay).await;

    a.get_mut().write_all(b"e2e4\n").await.unwrap();
    let mut line = String::new();
    b.read_line(&mut line).await.unwrap();
    assert_eq!(line, "e2e4\n");
    b.get_mut().write_all(b"e7e5\n").await.unwrap();
    line.clear();
    a.read_line(&mut line).await.unwrap();
    assert_eq!(line, "e7e5\n");

    a.get_mut().shutdown().await.unwrap();
    let mut rest = Vec::new();
    b.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
}

#[tokio::test]
async fn sessions_are_kept_apart_by_token() {
    let relay = start(Limits::default()).await;
    let other = "ffffffffffffffffffffffffffffffff";
    let ((mut a1, r1), (mut b1, r2), (mut a2, r3), (mut b2, r4)) = tokio::join!(
        open(relay, TOKEN),
        open(relay, other),
        open(relay, TOKEN),
        open(relay, other),
    );
    assert!([r1, r2, r3, r4].iter().all(|r| r == "PAIRED"));
    a1.get_mut().write_all(b"one\n").await.unwrap();
    b1.get_mut().write_all(b"two\n").await.unwrap();
    let (mut l1, mut l2) = (String::new(), String::new());
    a2.read_line(&mut l1).await.unwrap();
    b2.read_line(&mut l2).await.unwrap();
    assert_eq!((l1.as_str(), l2.as_str()), ("one\n", "two\n"));
}

#[tokio::test]
async fn a_lone_side_is_told_no_peer_came() {
    let relay = start(Limits {
        wait: Duration::from_millis(200),
        ..Limits::default()
    })
    .await;
    let (_, reply) = open(relay, TOKEN).await;
    assert_eq!(reply, "ERR no peer");
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let relay = start(Limits::default()).await;
    for token in ["short", "0123456789ABCDEF0123456789ABCDEF", ""] {
        let (_, reply) = open(relay, token).await;
        assert_eq!(reply, "ERR bad request", "{token:?}");
    }
}

#[tokio::test]
async fn a_third_connection_does_not_join_a_running_session() {
    let relay = start(Limits {
        wait: Duration::from_millis(300),
        ..Limits::default()
    })
    .await;
    let (_a, _b) = open_pair(relay).await;
    let (_, reply) = open(relay, TOKEN).await;
    assert_eq!(reply, "ERR no peer");
}

#[tokio::test]
async fn the_byte_limit_ends_the_session() {
    let relay = start(Limits {
        max_bytes: 1000,
        ..Limits::default()
    })
    .await;
    let (mut a, mut b) = open_pair(relay).await;
    a.get_mut().write_all(&[b'x'; 2000]).await.unwrap();
    assert!(closes(&mut b).await);
}

#[tokio::test]
async fn an_idle_session_is_closed() {
    let relay = start(Limits {
        idle: Duration::from_millis(300),
        ..Limits::default()
    })
    .await;
    let (mut a, mut b) = open_pair(relay).await;
    assert!(closes(&mut a).await);
    assert!(closes(&mut b).await);
}

#[tokio::test]
async fn the_session_limit_turns_new_sessions_away() {
    let relay = start(Limits {
        max_sessions: 1,
        ..Limits::default()
    })
    .await;
    let (_a, _b) = open_pair(relay).await;
    let (_, reply) = open(relay, "ffffffffffffffffffffffffffffffff").await;
    assert_eq!(reply, "ERR relay busy");
}
