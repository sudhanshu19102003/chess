use std::time::{Duration, Instant};

use chess_core::{Color, parse_square};
use chess_tui::{
    app::{App, Screen},
    clock::{self, Clock},
    layout,
    layout::Fit,
    render::Renderer,
};
use ratatui::{Terminal, backend::TestBackend};

fn midgame(t0: Instant) -> App {
    let mut app = App::new(t0);
    app.screen = Screen::Playing;
    assert!(app.play_uci("e2e4", t0));
    assert!(app.play_uci("d7d5", t0 + Duration::from_secs(1)));
    assert!(app.play_uci("e4d5", t0 + Duration::from_secs(2)));
    app.selected = Some(parse_square("g8").unwrap());
    app
}

/// Every terminal size from tiny to huge renders without panicking in both
/// pixel modes, and the layout stays inside the screen whenever it is shown.
#[test]
fn renders_at_every_size() {
    let t0 = Instant::now();
    let mut app = midgame(t0);
    let mut renderer = Renderer::new(true);
    let sizes = (1..=40).flat_map(|w| [(w * 6, 10), (w * 6, 30), (w * 6, 60), (w * 3, 90)]);
    for (w, h) in sizes.chain([(1, 1), (0, 0), (500, 150)]) {
        if let Some(l) = layout::compute(w as i32, h as i32) {
            assert!(l.g.x >= 0 && l.g.x + l.g.cols() <= w as i32, "board too wide at {w}x{h}");
            assert!(l.g.y >= 0 && l.g.y / 2 + l.g.rows() <= h as i32, "board too tall at {w}x{h}");
        }
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut fresh = App::new(t0);
        for hires in [false, true] {
            fresh.hires = hires;
            for screen in [Screen::Home, Screen::Countdown { go: t0 + Duration::from_secs(2) }] {
                fresh.screen = screen;
                term.draw(|f| renderer.draw(f, &fresh, t0)).unwrap();
            }
        }
        for (hires, anyway) in [(false, false), (true, false), (false, true), (true, true)] {
            app.hires = hires;
            app.play_anyway = anyway;
            for ms in [0, 90, 400] {
                term.draw(|f| renderer.draw(f, &app, t0 + Duration::from_millis(2000 + ms))).unwrap();
            }
        }
    }
}

#[test]
fn the_layout_needs_its_design_size() {
    let (c, r) = layout::needed();
    assert!(layout::compute(c as i32, r as i32).is_some());
    assert!(layout::compute(c as i32 - 10, r as i32).is_none());
    assert!(layout::compute(c as i32, r as i32 - 10).is_none());
    // Bigger windows get the same layout, centred.
    let big = layout::compute(c as i32 + 100, r as i32 + 40).unwrap();
    assert_eq!(big.g.s, layout::SQUARE);
}

#[test]
fn clock_runs_for_the_side_to_move() {
    let t0 = Instant::now();
    let mut c = Clock::new(4); // 3 | 2
    assert_eq!(c.running(), None);
    c.on_move(Color::White, t0);
    assert_eq!(c.running(), Some(Color::Black));
    assert_eq!(c.remaining(Color::Black, t0 + Duration::from_secs(10)), Duration::from_secs(170));
    c.on_move(Color::Black, t0 + Duration::from_secs(10));
    // Black banks 10s used and gains the 2s increment.
    assert_eq!(c.remaining(Color::Black, t0 + Duration::from_secs(60)), Duration::from_secs(172));
}

#[test]
fn clock_pauses_and_flags() {
    let t0 = Instant::now();
    let mut c = Clock::new(0); // 1 min
    c.on_move(Color::White, t0);
    c.toggle_pause(t0 + Duration::from_secs(20));
    assert_eq!(c.remaining(Color::Black, t0 + Duration::from_secs(500)), Duration::from_secs(40));
    c.toggle_pause(t0 + Duration::from_secs(500));
    c.tick(t0 + Duration::from_secs(541));
    assert_eq!(c.flagged, Some(Color::Black));
    assert_eq!(c.running(), None);
}

#[test]
fn clock_format() {
    assert_eq!(clock::format(Duration::from_secs(600)), "10:00");
    assert_eq!(clock::format(Duration::from_secs(65)), "1:05");
    assert_eq!(clock::format(Duration::from_millis(19_440)), "19.4");
}

#[test]
fn windows_that_are_too_small_or_too_big_get_a_message() {
    let (c, r) = layout::needed();
    let (c, r) = (c as i32, r as i32);
    assert_eq!(layout::fit(c, r), Fit::Good);
    assert_eq!(layout::fit(c - 1, r), Fit::TooSmall);
    assert_eq!(layout::fit(c, r - 1), Fit::TooSmall);
    // A bit of spare room is fine; a lot means zoom in.
    assert_eq!(layout::fit(c * 11 / 10, r * 11 / 10), Fit::Good);
    assert_eq!(layout::fit(c * 3 / 2, r * 3 / 2), Fit::TooBig);
    // Wide but not tall is still a fit: zooming in would not fit the rows.
    assert_eq!(layout::fit(c * 2, r), Fit::Good);
}

#[test]
fn enter_plays_anyway_in_a_big_window() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut app = App::new(Instant::now());
    let (c, r) = layout::needed();
    app.size = (c * 2, r * 2);
    assert_eq!(app.fit(), Fit::TooBig);
    app.on_key(KeyEvent::from(KeyCode::Right), Instant::now());
    assert_eq!(app.cursor, parse_square("e2").unwrap(), "board keys do nothing behind the message");
    app.on_key(KeyEvent::from(KeyCode::Enter), Instant::now());
    assert_eq!(app.fit(), Fit::Good);
}
