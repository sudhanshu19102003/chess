//! Renders the UI headlessly to a PNG, to check the look at any terminal
//! size without a real terminal. macOS only (uses system fonts).
//!
//!     cargo run --release --example snapshot -- <cols> <rows> <out.png> [home|drawn|countdown|start|midgame|captures|check|promotion|mate|lobby|code|connecting|failed|unavailable|online|dropped]
//!
//! Set `CHESS_PIXELS=octant` to render the high-resolution pieces.

use std::time::{Duration, Instant};

use chess_core::{Game, Position};
use chess_tui::{
    app::{App, Lobby, Mode, Pick, Promotion, Screen},
    net::{Failure, NetEvent, Request},
    octant,
    render::Renderer,
};
use fontdue::{Font, FontSettings};
use ratatui::{Terminal, backend::TestBackend, style::Color};

const CW: usize = 10;
const CH: usize = 20;

fn sq(name: &str) -> chess_core::Square {
    chess_core::parse_square(name).expect(name)
}

/// Plays `moves`, seven seconds apart, and returns the time after the last.
fn play(app: &mut App, t0: Instant, moves: &[&str]) -> Instant {
    let mut now = t0;
    for (i, m) in moves.iter().enumerate() {
        now = t0 + Duration::from_secs(7 * (i as u64 + 1));
        assert!(app.play_uci(m, now), "{m} is not legal here");
    }
    now
}

fn rgb(c: Color, default: [u8; 3]) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => default,
    }
}

fn load(path: &str) -> Font {
    Font::from_bytes(std::fs::read(path).expect(path), FontSettings::default()).expect(path)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let w: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(160);
    let h: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);
    let out = args.get(3).cloned().unwrap_or_else(|| "snapshot.png".into());
    let scene = args.get(4).cloned().unwrap_or_else(|| "midgame".into());

    let t0 = Instant::now();
    let mut app = App::new(t0);
    let mut now = t0 + Duration::from_millis(400);
    app.screen = Screen::Playing;
    match scene.as_str() {
        "home" => {
            app.screen = Screen::Home;
            app.mode = Mode::Offline;
        }
        "drawn" => {
            app.screen = Screen::Home;
            app.mode = Mode::PeerToPeer;
            app.drawn = Some("their move came 9.6 s later than their clock said; 4.4 s of lag was allowed".into());
        }
        "lobby" => {
            app.screen = Screen::Lobby;
            app.lobby = Lobby::Choose { pick: Pick::HostLan };
        }
        "code" => {
            app.screen = Screen::Lobby;
            app.lobby = Lobby::Code { input: "7-comet-pi".into(), error: false };
        }
        "connecting" => {
            app.screen = Screen::Lobby;
            app.server = "127.0.0.1".into();
            let stage = chess_net::connect::Stage::Code("7-comet-pilot".into());
            app.lobby = Lobby::Connecting { request: Request::Host(app.clock.offer()), stage, code: Some("7-comet-pilot".into()) };
        }
        "lan" => {
            app.screen = Screen::Lobby;
            let stage = chess_net::connect::Stage::Code("lan-42-comet-pilot".into());
            app.lobby = Lobby::Connecting { request: Request::HostLan(app.clock.offer()), stage, code: Some("lan-42-comet-pilot".into()) };
        }
        "failed" => {
            app.screen = Screen::Lobby;
            let message = "the game codes did not match: check the code, or someone tried to listen in".into();
            let failure = Failure { title: "COULD NOT CONNECT", message, detail: None };
            app.lobby = Lobby::Failed { request: Request::Join("7-comet-pilot".into()), failure };
        }
        "unavailable" => {
            app.screen = Screen::Lobby;
            let error = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
            let failure = Failure::from(&chess_net::connect::Error::NoServer { resolved: true, error });
            app.lobby = Lobby::Failed { request: Request::Host(app.clock.offer()), failure };
        }
        "online" | "dropped" => {
            // Playing Black over a relay, a few moves in.
            use chess_verify::{GameId, Session, SigningKey, TimeControl};
            let key = |b: u8| SigningKey::from_bytes(&[b; 32]);
            let tc = TimeControl { base_ms: 300_000, increment_ms: 0 };
            let me = Session::new(GameId([1; 16]), Position::starting(), chess_core::Color::Black, key(2), key(1).verifying_key(), tc);
            let mut them = Session::new(GameId([1; 16]), Position::starting(), chess_core::Color::White, key(1), key(2).verifying_key(), tc);
            app.screen = Screen::Lobby;
            let path = chess_net::Path { via: chess_net::Via::Relay, outgoing: true };
            app.on_net(NetEvent::Ready { session: Box::new(me), path, rtt: Duration::from_millis(48) }, t0);
            now = t0 + Duration::from_secs(4);
            app.tick(now);
            for (i, uci) in ["e2e4", "e7e5", "g1f3"].iter().enumerate() {
                now += Duration::from_secs(5);
                let mv = chess_core::Move::parse_uci(uci).unwrap();
                if i % 2 == 0 {
                    let bytes = them.play(mv, 5_000).unwrap().encode();
                    app.on_net(NetEvent::Link(chess_net::link::Event::Message { bytes, at: now }), now);
                } else {
                    assert!(app.make_move(mv, now));
                    let bytes = std::mem::take(&mut app.requests).into_iter().find_map(|r| match r { Request::Send(b) => Some(b), _ => None }).unwrap();
                    them.receive(&bytes).unwrap();
                }
            }
            if scene == "dropped" {
                app.on_net(NetEvent::Link(chess_net::link::Event::Closed("connection reset by peer".into())), now);
            }
            now += Duration::from_secs(2);
        }
        "copied" => {
            app.screen = Screen::Lobby;
            app.server = "127.0.0.1".into();
            let stage = chess_net::connect::Stage::Code("7-comet-pilot".into());
            app.lobby = Lobby::Connecting { request: Request::Host(app.clock.offer()), stage, code: Some("7-comet-pilot".into()) };
            app.copied = Some((now, true));
        }
        "offer" | "resigned" => {
            // White offered a draw with its third move; Black may take it.
            let moves = ["e2e4", "e7e5", "g1f3", "b8c6"];
            now = play(&mut app, t0, &moves) + Duration::from_secs(2);
            app.draw_offer = Some(chess_core::Color::White);
            app.play_uci("f1c4", now);
            now += Duration::from_secs(3);
            if scene == "resigned" {
                app.resigned = Some(chess_core::Color::Black);
                app.clock.halt(now);
            }
        }
        "typing" => {
            // "N" typed after a few moves: the knight moves, the first chosen.
            let moves = ["e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "g8f6"];
            now = play(&mut app, t0, &moves) + Duration::from_secs(3);
            app.input = Some(chess_tui::app::MoveInput { text: "N".into(), choice: 0 });
        }
        "countdown" => app.screen = Screen::Countdown { go: now + Duration::from_millis(2300) },
        "captures" => {
            // A game with several captures each way.
            let moves = ["e2e4", "d7d5", "e4d5", "d8d5", "b1c3", "d5a2", "a1a2", "c8g4", "d1g4", "b8c6", "g4g7", "f8g7", "c3d5", "g7b2", "c1b2"];
            now = play(&mut app, t0, &moves) + Duration::from_secs(2);
        }
        "midgame" => {
            // The knight on g5 is picked up, showing its moves and the capture on f7.
            let moves = ["e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "g8f6", "f3g5", "d7d5", "e4d5", "f6d5"];
            now = play(&mut app, t0, &moves) + Duration::from_secs(3);
            app.selected = Some(sq("g5"));
            app.cursor = sq("f7");
        }
        "check" => {
            // Black is in check; the g7 pawn may only block on g6.
            now = play(&mut app, t0, &["e2e4", "f7f6", "d1h5"]) + Duration::from_secs(3);
            app.selected = Some(sq("g7"));
            app.cursor = sq("g6");
        }
        "mate" => {
            let moves = ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"];
            now = play(&mut app, t0, &moves) + Duration::from_secs(3);
        }
        "promotion" => {
            app.game = Game::from_position(Position::from_fen("1n2k3/P7/8/8/8/8/5PPP/4K3 w - - 0 40").unwrap());
            app.selected = Some(sq("a7"));
            app.promotion = Some(Promotion { from: sq("a7"), to: sq("b8"), choice: 1 });
            app.cursor = sq("b8");
        }
        _ => {}
    }
    app.tick(now);
    // Explicit, not detected: this tool usually runs inside Ghostty.
    app.hires = std::env::var("CHESS_PIXELS").as_deref() == Ok("octant");

    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut renderer = Renderer::new(true);
    term.draw(|f| renderer.draw(f, &app, now)).unwrap();
    if let Some(n) = std::env::var("BENCH").ok().and_then(|v| v.parse::<u32>().ok()) {
        let start = Instant::now();
        for i in 0..n {
            let t = now + Duration::from_millis(33 * i as u64);
            term.draw(|f| renderer.draw(f, &app, t)).unwrap();
        }
        println!("{:.2} ms/frame", start.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
    let buf = term.backend().buffer().clone();

    let menlo = load("/System/Library/Fonts/Menlo.ttc");
    let symbols = load("/System/Library/Fonts/Apple Symbols.ttf");
    let size = 16.0;
    let ascent = menlo.horizontal_line_metrics(size).unwrap().ascent;

    let (iw, ih) = (w as usize * CW, h as usize * CH);
    let mut img = vec![0u8; iw * ih * 3];
    let mut put = |x: usize, y: usize, c: [u8; 3], a: f32| {
        if x < iw && y < ih {
            let i = (y * iw + x) * 3;
            for k in 0..3 {
                img[i + k] = (img[i + k] as f32 * (1.0 - a) + c[k] as f32 * a) as u8;
            }
        }
    };
    for cy in 0..h as usize {
        for cx in 0..w as usize {
            let cell = &buf[(cx as u16, cy as u16)];
            let fg = rgb(cell.fg, [220, 220, 220]);
            let bg = rgb(cell.bg, [0, 0, 0]);
            let sym = cell.symbol();
            let Some(ch) = sym.chars().next() else { continue };
            // Block characters (half blocks, quadrants, octants) are drawn as
            // their 2x4 pixel pattern; anything else as bg plus a glyph.
            let bits = octant::bits(ch).unwrap_or(0);
            for y in 0..CH {
                for x in 0..CW {
                    let bit = (y * 4 / CH) * 2 + x * 2 / CW;
                    let c = if bits >> bit & 1 == 1 { fg } else { bg };
                    put(cx * CW + x, cy * CH + y, c, 1.0);
                }
            }
            if octant::bits(ch).is_some() {
                continue;
            }
            let font = if menlo.lookup_glyph_index(ch) != 0 { &menlo } else { &symbols };
            let (m, bitmap) = font.rasterize(ch, size);
            let left = cx as i32 * CW as i32 + m.xmin + (CW as i32 - m.advance_width as i32) / 2;
            let top = cy as i32 * CH as i32 + 3 + ascent as i32 - m.height as i32 - m.ymin;
            for gy in 0..m.height {
                for gx in 0..m.width {
                    let a = bitmap[gy * m.width + gx] as f32 / 255.0;
                    let (x, y) = (left + gx as i32, top + gy as i32);
                    if x >= 0 && y >= 0 {
                        put(x as usize, y as usize, fg, a);
                    }
                }
            }
        }
    }

    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), iw as u32, ih as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&img).unwrap();
    println!("wrote {out} ({iw}x{ih})");
}
