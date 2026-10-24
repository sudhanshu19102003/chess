//! Peer-to-peer play through the app, with the network replaced by handing
//! each app's outgoing messages to the other: whose turn it is, the agreed
//! clock, flags, refused messages and the lobby.

use std::time::{Duration, Instant};

use chess_core::{Color, Move, Position, parse_square};
use chess_net::link::Event;
use chess_net::{Path, Via};
use chess_tui::app::{App, COUNTDOWN, Lobby, Mode, Screen};
use chess_tui::net::{NetEvent, Request};
use chess_tui::online::LagQuota;
use chess_verify::{GameId, Hash, MoveMsg, Session, SigningKey, TimeControl};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

const GAME: GameId = GameId([3; 16]);
const TC: TimeControl = TimeControl { base_ms: 60_000, increment_ms: 1_000 };
const RTT: Duration = Duration::from_millis(40);

fn key(color: Color) -> SigningKey {
    SigningKey::from_bytes(&[if color == Color::White { 1 } else { 2 }; 32])
}

/// An app for `me`, connected and past its countdown at `t0`.
fn player(me: Color, t0: Instant) -> App {
    let mut app = App::new(t0);
    app.screen = Screen::Lobby;
    let session = Session::new(GAME, Position::starting(), me, key(me), key(me.opponent()).verifying_key(), TC);
    let path = Path { via: Via::Public, outgoing: true };
    app.on_net(NetEvent::Ready { session: Box::new(session), path, rtt: RTT }, t0 - COUNTDOWN);
    app.tick(t0);
    assert_eq!(app.screen, Screen::Playing);
    app
}

/// Both players, White first.
fn pair(t0: Instant) -> (App, App) {
    (player(Color::White, t0), player(Color::Black, t0))
}

/// The signed messages `app` wants sent, taken from its requests.
fn sent(app: &mut App) -> Vec<Vec<u8>> {
    app.requests
        .drain(..)
        .filter_map(|r| match r {
            Request::Send(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

fn deliver(to: &mut App, bytes: Vec<u8>, at: Instant) {
    to.on_net(NetEvent::Link(Event::Message { bytes, at }), at);
}

fn click(app: &mut App, at: &str, now: Instant) {
    app.cursor = parse_square(at).unwrap();
    app.on_key(KeyEvent::from(KeyCode::Enter), now);
}

/// `mover` plays `uci` by keyboard at `now`, and the other side gets it.
fn play(mover: &mut App, other: &mut App, uci: &str, now: Instant) {
    let mv = Move::parse_uci(uci).unwrap();
    click(mover, &chess_core::name(mv.from), now);
    click(mover, &chess_core::name(mv.to), now);
    let msgs = sent(mover);
    assert_eq!(msgs.len(), 1, "{uci} was not played");
    deliver(other, msgs[0].clone(), now);
}

fn head(app: &App) -> Hash {
    app.online.as_ref().unwrap().session.chain().head()
}

#[test]
fn a_whole_game_over_the_link() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    assert!(!w.flipped && b.flipped);
    let moves = ["f2f3", "e7e5", "g2g4", "d8h4"];
    for (i, uci) in moves.iter().enumerate() {
        let now = t0 + Duration::from_secs(i as u64 + 1);
        if i % 2 == 0 { play(&mut w, &mut b, uci, now) } else { play(&mut b, &mut w, uci, now) }
    }
    for app in [&w, &b] {
        assert_eq!(app.result(), Some((Some(Color::Black), "checkmate")));
        assert_eq!(app.history.len(), 4);
    }
    assert_eq!(head(&w), head(&b));
}

#[test]
fn only_your_own_pieces_on_your_own_turn() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    // Black cannot move while White is to move, not even White's pieces.
    click(&mut b, "e2", t0);
    assert_eq!(b.selected, None);
    click(&mut b, "e7", t0);
    assert_eq!(b.selected, None);
    play(&mut w, &mut b, "e2e4", t0);
    click(&mut w, "d2", t0);
    assert_eq!(w.selected, None);
    click(&mut b, "e7", t0);
    assert!(b.selected.is_some());
}

#[test]
fn the_clock_is_the_agreed_one() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    let t1 = t0 + Duration::from_secs(5);
    play(&mut w, &mut b, "e2e4", t1);
    // White thought 5 s and gets the increment, on both screens.
    for app in [&w, &b] {
        assert_eq!(app.clock.remaining(Color::White, t1), Duration::from_secs(56));
    }
    // Black's time runs on both screens from the move.
    let t2 = t1 + Duration::from_secs(3);
    for app in [&mut w, &mut b] {
        app.tick(t2);
        assert_eq!(app.clock.remaining(Color::Black, t2), Duration::from_secs(57));
    }
}

#[test]
fn running_out_sends_a_flag() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    let late = t0 + Duration::from_secs(61);
    w.tick(late);
    let flag = sent(&mut w);
    assert_eq!(flag.len(), 1);
    deliver(&mut b, flag[0].clone(), late);
    for app in [&w, &b] {
        assert_eq!(app.result(), Some((Some(Color::Black), "time")));
        assert_eq!(app.clock.remaining(Color::White, late), Duration::ZERO);
        assert_eq!(app.clock.remaining(Color::Black, late), Duration::from_secs(60));
    }
    assert_eq!(head(&w), head(&b));
}

#[test]
fn a_silent_opponent_loses_on_time_here() {
    let t0 = Instant::now();
    let (_, mut b) = pair(t0);
    // White's minute and their lag quota must both pass first.
    let quota = LagQuota::new(TC).left();
    b.tick(t0 + Duration::from_secs(60) + quota - Duration::from_millis(10));
    assert_eq!(b.result(), None);
    b.tick(t0 + Duration::from_secs(60) + quota + Duration::from_millis(10));
    assert_eq!(b.result(), Some((Some(Color::Black), "time")));
    assert!(sent(&mut b).is_empty());
}

/// Asserts `app` drew its network game and went home, saying why.
fn drawn_and_home(app: &mut App, why: &str) {
    assert_eq!(app.screen, Screen::Home);
    assert!(app.online.is_none());
    assert_eq!(app.requests, [Request::Hangup]);
    let drawn = app.drawn.clone().unwrap();
    assert!(drawn.contains(why), "{drawn}");
    // The note goes with the next key.
    app.on_key(KeyEvent::from(KeyCode::Down), Instant::now());
    assert_eq!(app.drawn, None);
}

#[test]
fn a_refused_message_draws_the_game() {
    let t0 = Instant::now();
    let (w, mut b) = pair(t0);
    // An illegal move, honestly signed: refused, not played.
    let c = w.online.as_ref().unwrap().session.chain();
    let bad = MoveMsg::signed(&key(Color::White), GAME, 1, c.head(), Move::parse_uci("e2e5").unwrap(), 0, Hash([0; 32]));
    deliver(&mut b, bad.encode(), t0);
    drawn_and_home(&mut b, "illegal move");
    // So does anything that does not parse.
    let (_, mut b) = pair(t0);
    deliver(&mut b, vec![0xff; 3], t0);
    drawn_and_home(&mut b, "malformed");
}

/// White's session as a dishonest client would drive it, reporting
/// `think_ms` of its choosing.
fn white_claiming(think_ms: u32, uci: &str) -> Vec<u8> {
    let mut s = Session::new(GAME, Position::starting(), Color::White, key(Color::White), key(Color::Black).verifying_key(), TC);
    s.play(Move::parse_uci(uci).unwrap(), think_ms).unwrap().encode()
}

#[test]
fn lag_past_the_quota_draws_the_game() {
    let t0 = Instant::now();
    let quota = LagQuota::new(TC).left();
    // Within the quota: played as usual.
    let (_, mut b) = pair(t0);
    let at = t0 + Duration::from_secs(10);
    deliver(&mut b, white_claiming(10_000 - quota.as_millis() as u32, "e2e4"), at);
    assert_eq!(b.history.len(), 1);
    assert!(b.drawn.is_none() && b.online.is_some());
    // Past it: White says 0 s but took 10.
    let (_, mut b) = pair(t0);
    deliver(&mut b, white_claiming(0, "e2e4"), at);
    drawn_and_home(&mut b, "10.0 s later than their clock said");
}

#[test]
fn a_laggy_move_that_ends_the_game_keeps_its_result() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    for (i, uci) in ["f2f3", "e7e5", "g2g4"].iter().enumerate() {
        let now = t0 + Duration::from_secs(i as u64 + 1);
        if i % 2 == 0 { play(&mut w, &mut b, uci, now) } else { play(&mut b, &mut w, uci, now) }
    }
    // Black mates, claiming no time at all, long after.
    let late = t0 + Duration::from_secs(40);
    let mv = Move::parse_uci("d8h4").unwrap();
    let o = b.online.as_mut().unwrap();
    let msg = o.session.play(mv, 0).unwrap().encode();
    deliver(&mut w, msg, late);
    assert_eq!(w.result(), Some((Some(Color::Black), "checkmate")));
    assert!(w.drawn.is_none());
}

#[test]
fn the_lag_quota_refills_up_to_its_cap() {
    // 1 | 0: Lichess's 0.39 s a move, doubled.
    let mut q = LagQuota::new(TimeControl { base_ms: 60_000, increment_ms: 0 });
    let gain = Duration::from_millis(780);
    assert_eq!(q.left(), 3 * gain);
    assert!(q.spend(3 * gain));
    assert_eq!(q.left(), gain);
    assert!(!q.spend(gain + Duration::from_millis(1)));
    for _ in 0..20 {
        assert!(q.spend(Duration::ZERO));
    }
    assert_eq!(q.left(), 7 * gain);
    // Longer games get at most 2 s a move.
    assert_eq!(LagQuota::new(TimeControl { base_ms: 1_800_000, increment_ms: 0 }).left(), Duration::from_secs(6));
}

#[test]
fn a_hash_mismatch_drops_the_game_at_once() {
    let t0 = Instant::now();
    let (w, mut b) = pair(t0);
    let c = w.online.as_ref().unwrap().session.chain();
    let wrong = MoveMsg::signed(&key(Color::White), GAME, 1, c.head(), Move::parse_uci("e2e4").unwrap(), 0, Hash([5; 32]));
    deliver(&mut b, wrong.encode(), t0);
    let dropped = b.online.as_ref().unwrap().dropped.clone().unwrap();
    assert!(dropped.contains("0505"), "{dropped}");
    assert!(b.history.is_empty());
}

#[test]
fn a_lost_connection_drops_an_unfinished_game() {
    let t0 = Instant::now();
    let (_, mut b) = pair(t0);
    let t1 = t0 + Duration::from_secs(10);
    b.tick(t1);
    b.on_net(NetEvent::Link(Event::Closed("reset".into())), t1);
    // White's clock stops where it stood.
    b.tick(t1 + Duration::from_secs(5));
    assert_eq!(b.clock.remaining(Color::White, t1 + Duration::from_secs(5)), Duration::from_secs(50));
    assert!(b.over());
    assert_eq!(b.result(), None);
    assert!(b.online.as_ref().unwrap().dropped.as_ref().unwrap().contains("reset"));
}

#[test]
fn leaving_mid_game_takes_two_presses() {
    let t0 = Instant::now();
    let (mut w, _) = pair(t0);
    w.on_key(KeyEvent::from(KeyCode::Char('r')), t0);
    assert_eq!(w.screen, Screen::Playing);
    w.on_key(KeyEvent::from(KeyCode::Char('r')), t0);
    assert_eq!(w.screen, Screen::Home);
    assert!(w.online.is_none());
    assert_eq!(w.requests, [Request::Hangup]);
}

#[test]
fn the_lobby_hosts_and_joins() {
    let now = Instant::now();
    let press = |app: &mut App, code: KeyCode| app.on_key(KeyEvent::from(code), now);
    let mut app = App::new(now);
    app.mode = Mode::PeerToPeer;
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen, Screen::Lobby);
    // Host with the chosen time control.
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.requests, [Request::Host(app.clock.offer())]);
    assert!(matches!(app.lobby, Lobby::Connecting { .. }));
    // Esc hangs up.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.requests.last(), Some(&Request::Hangup));
    assert_eq!(app.lobby, Lobby::Choose { join: false });

    // Join: typing a code with a q in it does not quit.
    app.requests.clear();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    for c in "7 Quartz-quill".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert!(!app.quit);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.requests, [Request::Join("7-quartz-quill".into())]);

    // A failure can be retried.
    app.on_net(NetEvent::Failed("no answer".into()), now);
    assert!(matches!(&app.lobby, Lobby::Failed { message, .. } if message == "no answer"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.requests.last(), Some(&Request::Join("7-quartz-quill".into())));

    // Something that is not a code stays here.
    let mut app = App::new(now);
    app.screen = Screen::Lobby;
    app.lobby = Lobby::Code { input: "hello".into(), error: false };
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.lobby, Lobby::Code { input: "hello".into(), error: true });
    assert!(app.requests.is_empty());
}

#[test]
fn the_hosts_code_goes_to_the_clipboard() {
    use chess_net::connect::Stage;
    use chess_tui::net::{NetEvent, Request};
    let t0 = Instant::now();
    let mut app = App::new(t0);
    app.screen = Screen::Lobby;
    app.lobby = Lobby::Connecting { request: Request::Join(String::new()), stage: Stage::Server, code: None };
    app.on_net(NetEvent::Stage(Stage::Code("7-comet-pilot".into())), t0);
    assert_eq!(app.clipboard.take().as_deref(), Some("7-comet-pilot"));
    app.copied = Some((t0, true));
    assert_eq!(app.copied_showing(t0), Some(true));
    // C copies again.
    app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('c')), t0);
    assert_eq!(app.clipboard.as_deref(), Some("7-comet-pilot"));
}

fn press(app: &mut App, c: char, now: Instant) {
    app.on_key(KeyEvent::from(KeyCode::Char(c)), now);
}

/// Hands each of `from`'s outgoing messages to `to`.
fn pass(from: &mut App, to: &mut App, now: Instant) {
    for msg in sent(from) {
        deliver(to, msg, now);
    }
}

#[test]
fn resigning_takes_two_presses_and_ends_both_games() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    play(&mut w, &mut b, "e2e4", t0);
    press(&mut b, 'x', t0);
    assert!(sent(&mut b).is_empty(), "the first press only arms it");
    press(&mut b, 'x', t0);
    pass(&mut b, &mut w, t0);
    for app in [&w, &b] {
        assert_eq!(app.result(), Some((Some(Color::White), "resignation")));
        assert_eq!(app.clock.running(), None);
    }
    assert_eq!(head(&w), head(&b));
}

#[test]
fn resigning_on_their_turn_waits_for_their_move() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    press(&mut b, 'x', t0);
    press(&mut b, 'x', t0);
    assert!(sent(&mut b).is_empty());
    assert!(b.result().is_none());
    play(&mut w, &mut b, "e2e4", t0);
    // Black's resignation went out as soon as White's move arrived.
    pass(&mut b, &mut w, t0);
    for app in [&w, &b] {
        assert_eq!(app.result(), Some((Some(Color::White), "resignation")));
    }
    assert_eq!(head(&w), head(&b));
}

#[test]
fn a_draw_offered_then_accepted() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    // Black cannot offer on White's turn.
    press(&mut b, 'd', t0);
    assert!(sent(&mut b).is_empty());
    press(&mut w, 'd', t0);
    pass(&mut w, &mut b, t0);
    assert_eq!(b.draw_offer, Some(Color::White));
    play(&mut w, &mut b, "e2e4", t0);
    assert_eq!(b.draw_offer, Some(Color::White), "the offer stands after the offerer's move");
    press(&mut b, 'd', t0);
    pass(&mut b, &mut w, t0);
    for app in [&w, &b] {
        assert_eq!(app.result(), Some((None, "agreement")));
    }
    assert_eq!(head(&w), head(&b));
}

#[test]
fn moving_declines_and_early_acceptance_waits() {
    let t0 = Instant::now();
    let (mut w, mut b) = pair(t0);
    press(&mut w, 'd', t0);
    pass(&mut w, &mut b, t0);
    // Black accepts before White has moved: it goes after White's move.
    press(&mut b, 'd', t0);
    assert!(sent(&mut b).is_empty());
    play(&mut w, &mut b, "e2e4", t0);
    pass(&mut b, &mut w, t0);
    assert_eq!(w.result(), Some((None, "agreement")));

    // A fresh game: Black moves instead of accepting.
    let (mut w, mut b) = pair(t0);
    press(&mut w, 'd', t0);
    pass(&mut w, &mut b, t0);
    play(&mut w, &mut b, "e2e4", t0);
    play(&mut b, &mut w, "e7e5", t0);
    assert_eq!(w.draw_offer, None);
    assert_eq!(b.draw_offer, None);
    assert!(w.result().is_none());
}
