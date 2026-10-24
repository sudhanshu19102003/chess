pub mod app;
pub mod canvas;
pub mod clipboard;
pub mod clock;
pub mod layout;
pub mod net;
pub mod octant;
pub mod online;
pub mod panel;
pub mod pieces;
pub mod render;
pub mod sprite_sheet;
pub mod theme;

/// Truecolor unless the terminal says otherwise. `CHESS_COLORS=256` forces
/// the 256-colour fallback.
pub fn truecolor() -> bool {
    match std::env::var("CHESS_COLORS").as_deref() {
        Ok("256") => return false,
        Ok("truecolor") => return true,
        _ => {}
    }
    matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor") | Ok("24bit"))
}

/// Whether to draw pieces with Unicode 16 octants, at twice the board's
/// resolution. Only terminals known to draw octants themselves get them by
/// default; `CHESS_PIXELS=octant|half` overrides, and `g` toggles in game.
pub fn octants_supported() -> bool {
    match std::env::var("CHESS_PIXELS").as_deref() {
        Ok("octant") => return true,
        Ok("half") => return false,
        _ => {}
    }
    matches!(std::env::var("TERM_PROGRAM").as_deref(), Ok("ghostty"))
}
