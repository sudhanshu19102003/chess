//! The handshake over in-memory pipes: agreement, wrong codes, versions,
//! and every single altered byte.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chess_core::{Move, Position};
use chess_net::handshake::{self, Error, Offer, version};
use chess_net::{Established, TimeControl};
use chess_verify::{Received, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

const CODE: &str = "7-comet-pilot";
const FIVE_PLUS_THREE: TimeControl = TimeControl {
    base_ms: 300_000,
    increment_ms: 3_000,
};
/// Five plus three, colours by coin flip.
const FLIP: Offer = Offer {
    time_control: FIVE_PLUS_THREE,
    colour: None,
};

type Outcome = Result<Established<DuplexStream>, Error>;

async fn shake(host_code: &str, join_code: &str) -> (Outcome, Outcome) {
    let (a, b) = duplex(1 << 16);
    tokio::join!(
        handshake::host(a, host_code, FLIP),
        handshake::join(b, join_code)
    )
}

#[tokio::test]
async fn both_sides_agree_and_can_play() {
    let (host, join) = shake(CODE, CODE).await;
    let (mut host, mut join) = (host.unwrap(), join.unwrap());

    assert_eq!(host.game_id, join.game_id);
    assert_eq!(host.me, join.me.opponent());
    assert_eq!(host.opponent, join.key.verifying_key());
    assert_eq!(join.opponent, host.key.verifying_key());
    assert_eq!(
        (host.time_control, join.time_control),
        (FIVE_PLUS_THREE, FIVE_PLUS_THREE)
    );

    // Play 1. e4 e5 through the channel with the verify crate's sessions.
    let session = |e: &Established<DuplexStream>| {
        Session::new(
            e.game_id,
            Position::starting(),
            e.me,
            e.key.clone(),
            e.opponent,
            e.time_control,
        )
    };
    let (mut hs, mut js) = (session(&host), session(&join));
    let host_white = host.me == chess_core::Color::White;
    for (uci, by_white) in [("e2e4", true), ("e7e5", false)] {
        let mv = Move::parse_uci(uci).unwrap();
        if by_white == host_white {
            step(&mut host, &mut hs, &mut join, &mut js, mv).await;
        } else {
            step(&mut join, &mut js, &mut host, &mut hs, mv).await;
        }
    }
    assert_eq!(hs.chain().head(), js.chain().head());
}

/// `from` plays `mv` and sends it; `to` receives and checks it.
async fn step(
    from: &mut Established<DuplexStream>,
    from_session: &mut Session,
    to: &mut Established<DuplexStream>,
    to_session: &mut Session,
    mv: Move,
) {
    let msg = from_session.play(mv, 1_000).unwrap();
    from.channel.send(&msg.encode()).await.unwrap();
    let bytes = to.channel.recv().await.unwrap();
    assert_eq!(
        to_session.receive(&bytes).unwrap(),
        Received::Move { mv, think_ms: 1_000 }
    );
}

#[tokio::test]
async fn each_game_gets_its_own_id_and_both_colours_come_up() {
    let mut ids = std::collections::HashSet::new();
    let mut host_white = 0;
    for _ in 0..40 {
        let (host, _) = shake(CODE, CODE).await;
        let host = host.unwrap();
        assert!(ids.insert(host.game_id.0));
        host_white += usize::from(host.me == chess_core::Color::White);
    }
    // Each colour has a 2^-40 chance of never coming up.
    assert!((1..40).contains(&host_white), "{host_white}");
}

#[tokio::test]
async fn the_host_may_choose_its_colour() {
    for colour in [chess_core::Color::White, chess_core::Color::Black] {
        for _ in 0..8 {
            let (a, b) = duplex(1 << 16);
            let offer = Offer {
                colour: Some(colour),
                ..FLIP
            };
            let (host, join) = tokio::join!(handshake::host(a, CODE, offer), handshake::join(b, CODE));
            assert_eq!(host.unwrap().me, colour);
            assert_eq!(join.unwrap().me, colour.opponent());
        }
    }
}

#[tokio::test]
async fn a_wrong_code_fails_on_both_sides() {
    let (host, join) = shake(CODE, "7-comet-pirate").await;
    assert!(matches!(host, Err(Error::WrongCode)), "{host:?}");
    assert!(matches!(join, Err(Error::WrongCode)), "{join:?}");
}

#[tokio::test]
async fn a_different_version_is_refused_with_both_versions() {
    let (a, mut b) = duplex(1 << 16);
    let peer = async {
        let theirs = b"chess-p2p handshake=999 verify=1 engine=1";
        b.write_all(&(theirs.len() as u16).to_be_bytes())
            .await
            .unwrap();
        b.write_all(theirs).await.unwrap();
        b
    };
    let (host, _b) = tokio::join!(handshake::host(a, CODE, FLIP), peer);
    match host {
        Err(Error::Version { ours, theirs }) => {
            assert_eq!(ours, version());
            assert!(theirs.contains("handshake=999"));
        }
        other => panic!("{other:?}"),
    }
}

/// Which way a byte is altered.
#[derive(Clone, Copy, Debug)]
enum Way {
    HostToJoin,
    JoinToHost,
}

/// A handshake through a forwarder that may alter one byte. Returns both
/// outcomes and the bytes sent each way.
async fn through_middle(flip: Option<(Way, usize)>) -> (Outcome, Outcome, usize, usize) {
    let (host_end, middle_h) = duplex(1 << 16);
    let (join_end, middle_j) = duplex(1 << 16);
    let (mh_read, mh_write) = tokio::io::split(middle_h);
    let (mj_read, mj_write) = tokio::io::split(middle_j);
    let (to_join, to_host) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let flip_at = |way: fn(Way) -> bool| flip.filter(|f| way(f.0)).map(|f| f.1);
    let f1 = tokio::spawn(forward(
        mh_read,
        mj_write,
        to_join.clone(),
        flip_at(|w| matches!(w, Way::HostToJoin)),
    ));
    let f2 = tokio::spawn(forward(
        mj_read,
        mh_write,
        to_host.clone(),
        flip_at(|w| matches!(w, Way::JoinToHost)),
    ));
    let (host, join) = tokio::join!(
        handshake::host(host_end, CODE, FLIP),
        handshake::join(join_end, CODE)
    );
    f1.abort();
    f2.abort();
    (
        host,
        join,
        to_join.load(Ordering::SeqCst),
        to_host.load(Ordering::SeqCst),
    )
}

/// Forwards `from` to `to`, counting bytes and flipping the low bit of the
/// byte at `flip`.
async fn forward(
    from: tokio::io::ReadHalf<DuplexStream>,
    to: tokio::io::WriteHalf<DuplexStream>,
    count: Arc<AtomicUsize>,
    flip: Option<usize>,
) {
    let (mut from, mut to) = (from, to);
    let mut buf = [0u8; 1];
    while let Ok(1) = from.read(&mut buf).await {
        let at = count.fetch_add(1, Ordering::SeqCst);
        if flip == Some(at) {
            buf[0] ^= 1;
        }
        if to.write_all(&buf).await.is_err() {
            return;
        }
    }
}

/// Every byte either way is checked: altering any one makes the side that
/// receives it refuse (cut-off frames end in the handshake's timeout, which
/// the paused clock runs at once).
#[tokio::test(start_paused = true)]
async fn any_single_altered_byte_is_refused() {
    let (host, join, to_join, to_host) = through_middle(None).await;
    assert!(host.is_ok() && join.is_ok());
    assert!(to_join > 100 && to_host > 100, "{to_join} {to_host}");

    for (way, len) in [(Way::HostToJoin, to_join), (Way::JoinToHost, to_host)] {
        for at in 0..len {
            let (host, join, _, _) = through_middle(Some((way, at))).await;
            let receiver = match way {
                Way::HostToJoin => &join,
                Way::JoinToHost => &host,
            };
            assert!(receiver.is_err(), "{way:?} byte {at} went unnoticed");
        }
    }
}
