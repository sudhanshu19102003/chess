//! Two apps over the real network task, with a rendezvous server and relay
//! on loopback: host, join with the host's code, play a move each way.

use std::time::{Duration, Instant};

use chess_core::Color;
use chess_net::connect::{Servers, Stage};
use chess_net::{Limits, serve, serve_relay};
use chess_tui::app::{App, Lobby, Mode, Screen};
use chess_tui::net::Net;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use tokio::net::TcpListener;

/// One turn of the main loop: `net`'s events to `app`, `app`'s requests to `net`.
fn step(app: &mut App, net: &mut Net) {
    for event in net.poll() {
        app.on_net(event, Instant::now());
    }
    app.tick(Instant::now());
    for request in app.requests.drain(..) {
        net.request(request);
    }
}

/// Runs both loops until `done`.
fn until(a: (&mut App, &mut Net), b: (&mut App, &mut Net), done: impl Fn(&App, &App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(a.0, b.0) {
        assert!(Instant::now() < deadline, "timed out: {:?} / {:?}", a.0.lobby, b.0.lobby);
        step(a.0, a.1);
        step(b.0, b.1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::from(code), Instant::now());
}

#[test]
fn two_apps_meet_and_play() {
    let servers_rt = tokio::runtime::Runtime::new().unwrap();
    let servers = servers_rt.block_on(async {
        let (main, probe) = loop {
            let main = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = main.local_addr().unwrap().port();
            if let Ok(probe) = TcpListener::bind(("127.0.0.1", port + 1)).await {
                break (main, probe);
            }
        };
        let relay = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let servers = Servers {
            rendezvous: main.local_addr().unwrap().to_string(),
            relay: Some(relay.local_addr().unwrap().to_string()),
            force_relay: false,
        };
        tokio::spawn(serve(main, probe, None));
        tokio::spawn(serve_relay(relay, Limits::default()));
        servers
    });

    let now = Instant::now();
    let (mut host, mut joiner) = (App::new(now), App::new(now));
    let (mut host_net, mut join_net) = (Net::new(servers.clone()).unwrap(), Net::new(servers).unwrap());

    host.mode = Mode::PeerToPeer;
    press(&mut host, KeyCode::Enter);
    press(&mut host, KeyCode::Enter);
    until((&mut host, &mut host_net), (&mut joiner, &mut join_net), |h, _| {
        matches!(&h.lobby, Lobby::Connecting { stage: Stage::Code(_), .. })
    });
    let Lobby::Connecting { code: Some(code), .. } = host.lobby.clone() else { unreachable!() };

    joiner.screen = Screen::Lobby;
    joiner.lobby = Lobby::Code { input: code, error: false };
    press(&mut joiner, KeyCode::Enter);
    until((&mut host, &mut host_net), (&mut joiner, &mut join_net), |h, j| {
        h.screen == Screen::Playing && j.screen == Screen::Playing
    });
    let (h, j) = (host.online.as_ref().unwrap(), joiner.online.as_ref().unwrap());
    assert_eq!(h.me(), j.me().opponent());
    assert_eq!(host.clock.control().name(), joiner.clock.control().name());

    let (white, white_net, black, black_net) = if h.me() == Color::White {
        (&mut host, &mut host_net, &mut joiner, &mut join_net)
    } else {
        (&mut joiner, &mut join_net, &mut host, &mut host_net)
    };
    assert!(white.play_uci("e2e4", Instant::now()));
    until((&mut *white, &mut *white_net), (&mut *black, &mut *black_net), |_, b| b.history.len() == 1);
    assert!(black.play_uci("e7e5", Instant::now()));
    until((&mut *white, &mut *white_net), (&mut *black, &mut *black_net), |w, _| w.history.len() == 2);
    let head = |a: &App| a.online.as_ref().unwrap().session.chain().head();
    assert_eq!(head(white), head(black));
}
