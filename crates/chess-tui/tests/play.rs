//! Playing through the keyboard: only legal moves go through.

use std::time::Instant;

use chess_core::{Color, Game, Kind, Position, parse_square};
use chess_tui::app::{App, Screen};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

fn sq(name: &str) -> usize {
    parse_square(name).unwrap()
}

/// A game past its countdown, ready for moves.
fn game() -> App {
    let mut app = App::new(Instant::now());
    app.screen = Screen::Playing;
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::from(code), Instant::now());
}

/// Puts the cursor on `at` and presses Enter.
fn click(app: &mut App, at: &str) {
    app.cursor = sq(at);
    press(app, KeyCode::Enter);
}

#[test]
fn only_the_side_to_move_can_pick_up_a_piece() {
    let mut app = game();
    click(&mut app, "e7");
    assert_eq!(app.selected, None);
    click(&mut app, "e2");
    assert_eq!(app.selected, Some(sq("e2")));
    let mut targets: Vec<_> = app.targets().iter().map(|m| m.to).collect();
    targets.sort();
    assert_eq!(targets, vec![sq("e3"), sq("e4")]);
}

#[test]
fn an_illegal_drop_does_not_move() {
    let mut app = game();
    click(&mut app, "e2");
    click(&mut app, "e5");
    assert!(app.history.is_empty());
    assert_eq!(app.selected, None);
    assert_eq!(app.piece_at(sq("e2")).map(|p| p.kind), Some(Kind::Pawn));
}

#[test]
fn choosing_another_own_piece_switches_selection() {
    let mut app = game();
    click(&mut app, "e2");
    click(&mut app, "g1");
    assert_eq!(app.selected, Some(sq("g1")));
    click(&mut app, "f3");
    assert_eq!(app.history[0].san, "Nf3");
    assert_eq!(app.to_move(), Color::Black);
}

#[test]
fn promotion_asks_for_a_piece() {
    let mut app = game();
    app.game = Game::from_position(Position::from_fen("1n2k3/P7/8/8/8/8/8/4K3 w - - 0 1").unwrap());
    click(&mut app, "a7");
    click(&mut app, "b8");
    assert!(app.promotion.is_some(), "the picker opens");
    assert!(app.history.is_empty());
    // Esc puts the pawn back; it stays picked up.
    press(&mut app, KeyCode::Esc);
    assert!(app.promotion.is_none());
    click(&mut app, "b8");
    // White promotes at the top of the screen, so the picker runs down:
    // queen, rook, bishop, knight.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.history[0].san, "axb8=N");
    assert_eq!(app.piece_at(sq("b8")).map(|p| p.kind), Some(Kind::Knight));
}

#[test]
fn castling_slides_the_rook_too() {
    let mut app = game();
    app.game = Game::from_position(Position::from_fen("4k3/8/8/8/8/8/8/4K2R w K - 0 1").unwrap());
    click(&mut app, "e1");
    click(&mut app, "g1");
    assert_eq!(app.history[0].san, "O-O");
    assert_eq!(app.slide.as_ref().unwrap().movers.len(), 2);
    assert_eq!(app.piece_at(sq("f1")).map(|p| p.kind), Some(Kind::Rook));
}

#[test]
fn nothing_moves_after_checkmate() {
    let mut app = game();
    for m in ["f2f3", "e7e5", "g2g4", "d8h4"] {
        assert!(app.play_uci(m, Instant::now()));
    }
    assert_eq!(app.result(), Some((Some(Color::Black), "checkmate")));
    assert_eq!(app.clock.running(), None, "clocks stop");
    click(&mut app, "a2");
    assert_eq!(app.selected, None);
}

#[test]
fn the_home_screen_counts_down_then_starts_whites_clock() {
    use chess_tui::app::{COUNTDOWN, Mode};
    use std::time::Duration;
    let t0 = Instant::now();
    let mut app = App::new(t0);
    assert_eq!(app.screen, Screen::Home);
    // VS COMPUTER opens its setup first; Esc comes back.
    app.on_key(KeyEvent::from(KeyCode::Up), t0);
    assert_eq!(app.mode, Mode::Computer);
    app.on_key(KeyEvent::from(KeyCode::Enter), t0);
    assert_eq!(app.screen, Screen::Setup);
    app.on_key(KeyEvent::from(KeyCode::Esc), t0);
    assert_eq!(app.screen, Screen::Home);
    app.on_key(KeyEvent::from(KeyCode::Down), t0);
    app.on_key(KeyEvent::from(KeyCode::Enter), t0);
    assert_eq!(app.screen, Screen::Countdown { go: t0 + COUNTDOWN });
    assert_eq!(app.countdown(t0), Some(3));
    assert_eq!(app.countdown(t0 + Duration::from_millis(2500)), Some(1));
    // The board waits during the countdown.
    app.cursor = sq("e2");
    app.on_key(KeyEvent::from(KeyCode::Enter), t0 + Duration::from_secs(1));
    assert_eq!(app.selected, None);
    app.tick(t0 + Duration::from_secs(2));
    assert_eq!(app.clock.running(), None);
    app.tick(t0 + Duration::from_secs(4));
    assert_eq!(app.screen, Screen::Playing);
    assert_eq!(app.clock.running(), Some(Color::White));
    assert_eq!(app.clock.remaining(Color::White, t0 + Duration::from_secs(4)), Duration::from_secs(599));
    // R goes back to the menu.
    app.on_key(KeyEvent::from(KeyCode::Char('r')), t0 + Duration::from_secs(5));
    assert_eq!(app.screen, Screen::Home);
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c));
    }
}

#[test]
fn a_typed_piece_and_square_plays_the_move() {
    let mut app = game();
    typed(&mut app, "N");
    let found: Vec<String> = app.suggestions().into_iter().map(|s| s.san).collect();
    assert_eq!(found, ["Na3", "Nc3", "Nf3", "Nh3"]);
    typed(&mut app, "f3");
    assert_eq!(app.history[0].san, "Nf3");
    assert!(app.input.is_none());
    // Pawns go by P, and a capture can be typed with or without the x.
    typed(&mut app, "Pe5");
    typed(&mut app, "Pe4");
    typed(&mut app, "Pd5");
    typed(&mut app, "Pxd5");
    assert_eq!(app.history.last().unwrap().san, "exd5");
}

#[test]
fn a_typed_move_with_several_matches_waits_for_enter() {
    let mut app = game();
    app.game = Game::from_position(Position::from_fen("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1").unwrap());
    typed(&mut app, "Nd2");
    assert!(app.history.is_empty(), "two knights reach d2");
    let found: Vec<String> = app.suggestions().into_iter().map(|s| s.san).collect();
    assert_eq!(found, ["Nbd2", "Nfd2"]);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.history[0].san, "Nfd2");
}

#[test]
fn typed_moves_show_five_and_esc_gives_up() {
    let mut app = game();
    typed(&mut app, "P");
    assert_eq!(app.suggestions().len(), 16);
    // q types while a move is being typed, rather than quitting.
    typed(&mut app, "q");
    assert!(!app.quit);
    assert!(app.suggestions().is_empty());
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Esc);
    assert!(app.input.is_none());
    assert!(app.history.is_empty());
}

#[test]
fn typed_promotion_and_castling() {
    let mut app = game();
    app.game = Game::from_position(Position::from_fen("4k3/P7/8/8/8/8/8/4K2R w K - 0 1").unwrap());
    typed(&mut app, "Pa8");
    assert_eq!(app.suggestions().len(), 4, "one per piece");
    typed(&mut app, "=N");
    assert_eq!(app.history[0].san, "a8=N");
    typed(&mut app, "Kd7");
    typed(&mut app, "O-O");
    assert_eq!(app.history[2].san, "O-O");
}

mod mouse {
    use super::*;
    use chess_tui::layout;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    /// The middle cell of a square.
    fn cell(app: &App, name: &str) -> (u16, u16) {
        let g = layout::compute(app.size.0 as i32, app.size.1 as i32).unwrap().g;
        let (x, y) = g.square_px(sq(name), app.flipped);
        ((x + g.s / 2) as u16, ((y + g.s / 2) / 2) as u16)
    }

    fn mouse(app: &mut App, kind: MouseEventKind, at: (u16, u16)) {
        let ev = MouseEvent { kind, column: at.0, row: at.1, modifiers: KeyModifiers::NONE };
        app.on_mouse(ev, Instant::now());
    }

    fn tap(app: &mut App, name: &str) {
        let at = cell(app, name);
        mouse(app, MouseEventKind::Down(MouseButton::Left), at);
        mouse(app, MouseEventKind::Up(MouseButton::Left), at);
    }

    #[test]
    fn every_cell_of_a_square_is_that_square() {
        let app = game();
        let g = layout::compute(app.size.0 as i32, app.size.1 as i32).unwrap().g;
        for s in 0..64 {
            let (x, y) = g.square_px(s, false);
            for (col, row) in [(x, y / 2), (x + g.s - 1, (y + g.s - 1) / 2)] {
                assert_eq!(g.square_at_cell(col, row, false), Some(s));
            }
        }
        assert_eq!(g.square_at_cell(g.x, g.y / 2, false), None, "the frame");
    }

    #[test]
    fn click_then_click_moves() {
        let mut app = game();
        tap(&mut app, "g1");
        assert_eq!(app.selected, Some(sq("g1")));
        tap(&mut app, "g1");
        assert_eq!(app.selected, None, "a second click puts it down");
        tap(&mut app, "g1");
        tap(&mut app, "f3");
        assert_eq!(app.history[0].san, "Nf3");
    }

    #[test]
    fn drag_and_drop_moves() {
        let mut app = game();
        let at = cell(&app, "e2");
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
        let at = cell(&app, "e3");
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), at);
        let at = cell(&app, "e4");
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), at);
        assert!(app.drag.is_some_and(|d| d.moved));
        let at = cell(&app, "e4");
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), at);
        assert_eq!(app.history[0].san, "e4");
        assert!(app.slide.is_none(), "a dropped piece is already there");
        // An illegal drop puts the piece back.
        let at = cell(&app, "e7");
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
        let at = cell(&app, "e4");
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), at);
        assert_eq!(app.history.len(), 1);
        assert_eq!(app.selected, None);
    }

    #[test]
    fn clicking_a_picker_tile_promotes() {
        let mut app = game();
        app.game = Game::from_position(Position::from_fen("1n2k3/P7/8/8/8/8/8/4K3 w - - 0 1").unwrap());
        tap(&mut app, "a7");
        tap(&mut app, "a8");
        assert!(app.promotion.is_some());
        // Queen, rook, bishop, knight down the a-file.
        tap(&mut app, "a6");
        assert_eq!(app.history[0].san, "a8=B");
    }

    #[test]
    fn home_buttons_click() {
        use chess_tui::app::{Lobby, Mode};
        use chess_tui::panel::{HOME_W, home_origin};
        let mut app = App::new(Instant::now());
        let (x, y) = home_origin(app.size.0 as i32, app.size.1 as i32);
        // The middle of the second button: peer to peer.
        let at = ((x + HOME_W / 2) as u16, (y + 13) as u16);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
        assert_eq!(app.mode, Mode::PeerToPeer);
        assert_eq!(app.screen, Screen::Lobby);
        // Then the second lobby button: host on this network.
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
        assert!(app.lobby.lan());
        // Back, and the third: join.
        app.on_key(KeyCode::Esc.into(), Instant::now());
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), (at.0, at.1 + 4));
        assert!(matches!(app.lobby, Lobby::Code { .. }));
    }
}

#[test]
fn offline_resign_and_draw() {
    let mut app = game();
    // The side to move resigns, with a second press.
    press(&mut app, KeyCode::Char('x'));
    assert!(app.result().is_none());
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(app.result(), Some((Some(Color::Black), "resignation")));

    // White offers and moves; Black accepts.
    let mut app = game();
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.draw_offer, Some(Color::White));
    click(&mut app, "e2");
    click(&mut app, "e4");
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.result(), Some((None, "agreement")));
    click(&mut app, "e7");
    assert_eq!(app.selected, None, "nothing moves after the draw");

    // Black moves instead: the offer lapses.
    let mut app = game();
    press(&mut app, KeyCode::Char('d'));
    app.play_uci("e2e4", Instant::now());
    app.play_uci("e7e5", Instant::now());
    assert_eq!(app.draw_offer, None);
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.draw_offer, Some(Color::White), "a fresh offer");
}

#[test]
fn auto_flip_turns_the_board_after_the_piece_lands() {
    let mut app = game();
    assert!(app.auto_flip && !app.flipped);
    let t = Instant::now();
    click(&mut app, "e2");
    click(&mut app, "e4");
    // The move stays in view while the piece slides and for a moment after.
    app.tick(t + std::time::Duration::from_millis(100));
    assert!(!app.flipped);
    app.tick(t + std::time::Duration::from_secs(2));
    assert!(app.flipped, "Black to move, so Black at the bottom");

    let t = Instant::now();
    click(&mut app, "e7");
    click(&mut app, "e5");
    app.tick(t + std::time::Duration::from_secs(2));
    assert!(!app.flipped, "White to move again");
}

#[test]
fn a_turns_auto_flip_off_and_on() {
    let mut app = game();
    press(&mut app, KeyCode::Char('a'));
    assert!(!app.auto_flip);
    let t = Instant::now();
    click(&mut app, "e2");
    click(&mut app, "e4");
    app.tick(t + std::time::Duration::from_secs(2));
    assert!(!app.flipped, "off: the board stays put");

    // Turning it back on turns the board to the side to move at once.
    press(&mut app, KeyCode::Char('a'));
    app.tick(Instant::now());
    assert!(app.auto_flip && app.flipped);
}

#[test]
fn with_auto_flip_a_new_game_starts_from_whites_side() {
    let mut app = game();
    app.flipped = true;
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(app.screen, Screen::Home);
    press(&mut app, KeyCode::Enter);
    assert!(!app.flipped);
}

/// A VS COMPUTER game up to the end of its countdown, the engine ready.
fn versus(colour_presses: usize) -> (App, Instant) {
    use chess_tui::app::{COUNTDOWN, Mode};
    use chess_tui::engine::{Event, Request};
    let t0 = Instant::now();
    let mut app = App::new(t0);
    app.mode = Mode::Computer;
    app.on_key(KeyEvent::from(KeyCode::Enter), t0);
    assert_eq!(app.screen, Screen::Setup);
    assert_eq!(std::mem::take(&mut app.engine_requests), [Request::Prepare]);
    // Not ready yet: Enter waits.
    app.on_key(KeyEvent::from(KeyCode::Enter), t0);
    assert_eq!(app.screen, Screen::Setup);
    app.on_engine(Event::Progress { got: 10, total: 80 }, t0);
    app.on_engine(Event::Ready { name: "Stockfish 19".into() }, t0);
    app.on_key(KeyEvent::from(KeyCode::Char('2')), t0);
    app.on_key(KeyEvent::from(KeyCode::Down), t0);
    for _ in 0..colour_presses {
        app.on_key(KeyEvent::from(KeyCode::Right), t0);
    }
    app.on_key(KeyEvent::from(KeyCode::Enter), t0);
    assert!(matches!(app.screen, Screen::Countdown { .. }));
    let go = t0 + COUNTDOWN;
    app.tick(go);
    (app, go)
}

#[test]
fn the_computer_answers_the_players_moves() {
    use chess_tui::engine::{Event, Request};
    let (mut app, go) = versus(0);
    assert_eq!(app.versus.as_ref().unwrap().human, Color::White);
    assert_eq!(app.versus.as_ref().unwrap().level, 1);
    assert!(app.engine_requests.is_empty(), "White moves first");
    assert!(app.play_uci("e2e4", go));
    // The computer's pieces are not the player's.
    app.cursor = sq("e7");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected, None);
    // Asked once the piece has landed.
    let later = go + std::time::Duration::from_secs(1);
    app.tick(later);
    app.tick(later);
    let think = match std::mem::take(&mut app.engine_requests).as_slice() {
        [Request::Think(t)] => t.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(think.moves, ["e2e4"]);
    assert!(think.new_game);
    // A stale or illegal answer is not played.
    app.on_engine(Event::Move { id: think.id + 1, uci: "e7e5".into() }, later);
    assert_eq!(app.history.len(), 1);
    app.on_engine(Event::Move { id: think.id, uci: "e7e5".into() }, later);
    assert_eq!(app.history.len(), 2);
    assert_eq!(app.history[1].san, "e5");
    assert!(app.play_uci("g1f3", later));
}

#[test]
fn playing_black_the_computer_opens_and_the_board_turns() {
    use chess_tui::engine::{Event, Request};
    let (mut app, go) = versus(1);
    assert_eq!(app.versus.as_ref().unwrap().human, Color::Black);
    assert!(app.flipped);
    let id = match std::mem::take(&mut app.engine_requests).as_slice() {
        [Request::Think(t)] => t.id,
        other => panic!("{other:?}"),
    };
    assert!(!app.play_uci("e2e4", go), "not the player's pieces");
    app.on_engine(Event::Move { id, uci: "d2d4".into() }, go);
    assert!(app.play_uci("d7d5", go));
    // Leaving stops the search.
    app.on_key(KeyEvent::from(KeyCode::Char('r')), go);
    assert_eq!(app.screen, Screen::Home);
    assert!(app.engine_requests.contains(&Request::Stop));
}

#[test]
fn an_engine_failure_stops_the_game() {
    use chess_tui::engine::Event;
    let (mut app, go) = versus(0);
    app.on_engine(Event::Failed("Stockfish stopped unexpectedly".into()), go);
    assert!(app.over());
    assert!(!app.play_uci("e2e4", go));
}
