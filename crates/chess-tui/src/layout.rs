//! The one layout the UI is designed for: the board on 10-pixel squares with
//! a 48-column panel beside it, centred in the window. A window too small for
//! it gets a "zoom out" message instead, and one much bigger a "zoom in"
//! message, since the board would look small. Board measurements are in canvas
//! pixels: one pixel is one column wide and half a row tall.

use chess_core::Square;

/// Square size. With octants (2x finer) a square is 20 pixels, which fits
/// the sprite sheet's 16x16 pieces exactly at 1x.
pub const SQUARE: i32 = 10;
const FRAME: i32 = 4;
const SLAB: i32 = 2;
const SHADOW: i32 = 4;
const PANEL_W: i32 = 48;
const GAP: i32 = 4;
/// Rows of the clock strips above and below the board.
const STRIP: i32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct BoardGeom {
    /// Square size.
    pub s: i32,
    /// Frame thickness.
    pub f: i32,
    /// Height of the board's front edge below the frame.
    pub slab: i32,
    /// Drop-shadow offset and blur.
    pub shadow: i32,
    /// Top-left of the outer frame.
    pub x: i32,
    pub y: i32,
}

const BOARD: BoardGeom = BoardGeom { s: SQUARE, f: FRAME, slab: SLAB, shadow: SHADOW, x: 0, y: 0 };

impl BoardGeom {
    /// Width and height of the framed board.
    pub fn outer(&self) -> i32 {
        8 * self.s + 2 * self.f
    }

    /// Terminal columns used, including the shadow.
    pub fn cols(&self) -> i32 {
        self.outer() + self.shadow
    }

    /// Terminal rows used by the board and its front edge.
    pub fn body_rows(&self) -> i32 {
        (self.outer() + self.slab) / 2
    }

    /// Terminal rows used, including the shadow.
    pub fn rows(&self) -> i32 {
        (self.outer() + self.slab + self.shadow) / 2
    }

    /// Screen (column, row) of a square, 0..8 from the top-left.
    pub fn screen_cell(sq: Square, flipped: bool) -> (i32, i32) {
        let (file, rank) = (chess_core::file_of(sq) as i32, chess_core::rank_of(sq) as i32);
        if flipped { (7 - file, rank) } else { (file, 7 - rank) }
    }

    pub fn square_at_screen(col: i32, row: i32, flipped: bool) -> Square {
        let (file, rank) = if flipped { (7 - col, row) } else { (col, 7 - row) };
        chess_core::square(file as usize, rank as usize)
    }

    /// The square under a terminal cell, if any. Squares start on even
    /// pixel rows, so a cell is never split between two.
    pub fn square_at_cell(&self, col: i32, row: i32, flipped: bool) -> Option<Square> {
        let (x, y) = (col - self.x - self.f, 2 * row - self.y - self.f);
        let n = 8 * self.s;
        ((0..n).contains(&x) && (0..n).contains(&y)).then(|| Self::square_at_screen(x / self.s, y / self.s, flipped))
    }

    /// The same board measured on a canvas `k` times finer.
    pub fn scaled(&self, k: i32) -> Self {
        BoardGeom {
            s: self.s * k,
            f: self.f * k,
            slab: self.slab * k,
            shadow: self.shadow * k,
            x: self.x * k,
            y: self.y * k,
        }
    }

    /// Top-left pixel of a square.
    pub fn square_px(&self, sq: Square, flipped: bool) -> (i32, i32) {
        let (c, r) = Self::screen_cell(sq, flipped);
        (self.x + self.f + c * self.s, self.y + self.f + r * self.s)
    }
}

/// Cell rectangle relative to the drawing area.
#[derive(Clone, Copy, Debug)]
pub struct Cells {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub g: BoardGeom,
    pub panel: Cells,
    /// The clock strips: above the board, then below it.
    pub strips: [Cells; 2],
}

/// Columns and rows the board, its strips and the panel take up. The
/// bottom strip sits over the board's drop shadow.
fn content() -> (i32, i32) {
    (BOARD.cols() + GAP + PANEL_W, STRIP + BOARD.body_rows() + STRIP)
}

/// The terminal size the layout is designed for: its content plus a
/// one-cell margin.
pub fn needed() -> (u16, u16) {
    let (w, h) = content();
    (w as u16 + 2, h as u16 + 1)
}

/// How a window suits the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Too small for the layout: the user should zoom out.
    TooSmall,
    /// Fits.
    Good,
    /// At least 20% bigger than needed both ways, room for a couple more
    /// zoom steps in, so the board would look small: zoom in.
    TooBig,
}

pub fn fit(w: i32, h: i32) -> Fit {
    let (c, r) = needed();
    let (c, r) = (c as i32, r as i32);
    if w < c || h < r {
        Fit::TooSmall
    } else if w * 5 >= c * 6 && h * 5 >= r * 6 {
        Fit::TooBig
    } else {
        Fit::Good
    }
}

/// The layout centred in a `w` x `h` window, or `None` if it does not fit.
pub fn compute(w: i32, h: i32) -> Option<Layout> {
    let (cw, ch) = content();
    if w < cw || h < ch {
        return None;
    }
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let g = BoardGeom { x: x0, y: 2 * (y0 + STRIP), ..BOARD };
    let panel = Cells { x: x0 + g.cols() + GAP, y: y0, w: PANEL_W, h: ch };
    let strip = |y| Cells { x: x0, y, w: g.outer(), h: STRIP };
    Some(Layout { g, panel, strips: [strip(y0), strip(y0 + STRIP + g.body_rows())] })
}
