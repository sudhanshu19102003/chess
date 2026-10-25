//! The real Stockfish: downloads the release build into a scratch home
//! folder, checks it, and plays a few moves over UCI. It needs the internet
//! and fetches about 80 MB, so it is ignored by default:
//! `cargo test -p chess-tui --test engine -- --ignored`.

use std::time::{Duration, Instant};

use chess_core::{Game, Move};
use chess_tui::engine::{self, Engine, Event, Request, Think};

/// Waits for the next event that is not download progress.
fn next(engine: &mut Engine, limit: Duration) -> Event {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(event) = engine.poll().into_iter().find(|e| !matches!(e, Event::Progress { .. })) {
            return event;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no answer from the engine");
}

#[test]
#[ignore]
fn downloads_stockfish_and_plays() {
    let home = std::env::temp_dir().join(format!("chess-engine-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs one test, before any other thread starts.
    unsafe {
        for var in ["HOME", "XDG_DATA_HOME", "LOCALAPPDATA"] {
            std::env::set_var(var, &home);
        }
        std::env::remove_var("CHESS_STOCKFISH");
    }
    assert_eq!(engine::installed(), None);

    let mut engine = Engine::new();
    engine.request(Request::Prepare);
    match next(&mut engine, Duration::from_secs(600)) {
        Event::Ready { name } => assert!(name.starts_with("Stockfish"), "{name}"),
        other => panic!("{other:?}"),
    }
    assert!(engine::installed().is_some());

    // Both sides played by the engine, at every level.
    let mut game = Game::new();
    for (ply, level) in (0..engine::LEVELS.len()).cycle().take(16).enumerate() {
        let think = Think {
            id: ply as u64,
            new_game: ply == 0,
            moves: game.moves().iter().map(|m| m.uci()).collect(),
            level,
            left_ms: [60_000, 60_000],
            increment_ms: 0,
        };
        engine.request(Request::Think(think));
        match next(&mut engine, Duration::from_secs(30)) {
            Event::Move { id, uci } => {
                assert_eq!(id, ply as u64);
                assert!(game.play(Move::parse_uci(&uci).unwrap()), "{uci} at ply {ply}");
            }
            other => panic!("{other:?}"),
        }
        if game.outcome().is_some() {
            break;
        }
    }

    // A search abandoned for a new one only answers the new one.
    let think = |id| Think {
        id,
        new_game: true,
        moves: vec![],
        level: 7,
        left_ms: [600_000, 600_000],
        increment_ms: 0,
    };
    engine.request(Request::Think(think(100)));
    engine.request(Request::Stop);
    engine.request(Request::Think(think(101)));
    match next(&mut engine, Duration::from_secs(60)) {
        Event::Move { id, .. } => assert_eq!(id, 101),
        other => panic!("{other:?}"),
    }

    drop(engine);
    std::fs::remove_dir_all(&home).unwrap();
}
