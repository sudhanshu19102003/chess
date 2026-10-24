//! Piece art, in the style of the sprite sheet in `sprite_sheet` ("Chess
//! Asset 16bit pixel art" by gchoc, CC0). With octants a square is 20 pixels
//! and gets the sheet's 16x16 sprites as drawn; on half blocks it is 10
//! pixels and gets a reduced 7x8 version drawn here by hand in the sheet's
//! palette.

use chess_core::{Color as Side, Kind};
use crate::canvas::{self, Mask, Rgb, Sprite, rgb};
use crate::theme;
use crate::sprite_sheet as sheet;

/// The six pieces redrawn 7x8 for half-block squares. Characters are sheet
/// palette keys in White's colours: `a` outline, `b` light, `d` mid, `c`
/// dark, `e`-`j` the queen's silver and amethyst, `k`-`o` the king's gold
/// and rubies.
fn small(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Pawn => &[".......", "..aaa..", ".adbba.", "..ada..", ".abcba.", ".adbda.", "abcccba", ".aaaaa."],
        Kind::Rook => &[".aa.aa.", "abbabba", "aaacaaa", ".adbda.", ".acdca.", ".addda.", "abdddba", ".aaaaa."],
        Kind::Knight => &["...a...", "..aca..", ".abcba.", "abdcdba", "adcdaaa", ".addda.", "abdddba", ".aaaaa."],
        Kind::Bishop => &["...a...", "..afa..", ".abcba.", ".adada.", "..ada..", ".acdca.", "abdddba", ".aaaaa."],
        Kind::Queen => &["..aaa..", ".aeiea.", "agihiga", ".ajgja.", "..aca..", ".addda.", "abdddba", ".aaaaa."],
        Kind::King => &["..aaa..", ".alkla.", "akmnmka", ".aokoa.", "..aca..", ".addda.", "abdddba", ".aaaaa."],
    }
}

fn sheet_sprite(kind: Kind, side: Side) -> &'static [&'static str] {
    let index = match kind {
        Kind::Rook => 0,
        Kind::Knight => 1,
        Kind::Bishop => 2,
        Kind::Queen => 3,
        Kind::King => 4,
        Kind::Pawn => 5,
    };
    match side {
        Side::White => &sheet::WHITE[index],
        Side::Black => &sheet::BLACK[index],
    }
}

/// Black's body tones, as the sheet pairs them: light, mid and dark. The
/// sheet's own Black sprites never use White's keys, so this is safe for them.
fn recolour(key: u8, side: Side) -> u8 {
    match (side, key) {
        (Side::Black, b'b') => b'p',
        (Side::Black, b'd') => b'r',
        (Side::Black, b'c') => b'q',
        _ => key,
    }
}

/// The art for a square of size `s`: the sheet's sprites when they fit
/// with a pixel to spare, the 7x8 set otherwise.
fn pick(kind: Kind, side: Side, s: i32) -> &'static [&'static str] {
    if s - 2 >= sheet::SIZE as i32 { sheet_sprite(kind, side) } else { small(kind) }
}

/// A rendered piece: the sprite, its soft shadow, and the padding both
/// carry around the square (sprite pixel (pad, pad) is the square's corner).
pub struct PieceArt {
    pub sprite: Sprite,
    pub shadow: Mask,
    pub pad: i32,
}

/// Renders a piece for squares of size `s` on a canvas whose terminal cells
/// are `cell` = (width, height) pixels: (1, 2) for half blocks, (2, 4) for
/// octants. A cell has only two colours, so on octant cells the sprite is
/// nudged by up to a cell to the offset where the fewest pixels lose their
/// colour, which keeps outlines and tips intact.
/// The drawn part of a sprite, without its empty border.
fn cropped(rows: &[&str]) -> Vec<Vec<u8>> {
    let used: Vec<&[u8]> = rows.iter().map(|r| r.as_bytes()).filter(|r| r.iter().any(|&b| b != b'.')).collect();
    let w = used.first().map_or(0, |r| r.len());
    let col_used = |x: usize| used.iter().any(|r| r[x] != b'.');
    let (x0, x1) = ((0..w).find(|&x| col_used(x)).unwrap_or(0), (0..w).rev().find(|&x| col_used(x)).unwrap_or(0));
    used.iter().map(|r| r[x0..=x1].to_vec()).collect()
}

pub fn render(kind: Kind, side: Side, s: i32, cell: (i32, i32)) -> PieceArt {
    let blur = (s / 12).max(1);
    let pad = 1 + blur;
    let w = s + 2 * pad;
    let mut px = vec![[0.0f32; 4]; (w * w) as usize];

    let grid = cropped(pick(kind, side, s));
    let (width, height) = (grid.first().map_or(0, |r| r.len()) as i32, grid.len() as i32);
    let margin = ((s - height) / 2).min((s / 12).max(1)).max(0);
    // Centred, standing just above the square's bottom edge...
    let natural = ((s - width) / 2, s - margin - height);
    // ...then nudged to wherever its cells lose least to the two-colour
    // limit, tried on both square colours.
    let (left, top) = if cell.0 * cell.1 > 2 {
        let colour = |key: u8| {
            let i = sheet::KEYS.iter().position(|&c| c == recolour(key, side))?;
            let [r, g, b, _] = sheet::PALETTE[i].map(|v| v as f32 / 255.0);
            Some(rgb(r, g, b))
        };
        let at = |left: i32, top: i32, x: i32, y: i32| {
            let (sx, sy) = (x - left, y - top);
            if sx < 0 || sy < 0 || sx >= width || sy >= height {
                return None;
            }
            colour(grid[sy as usize][sx as usize])
        };
        let loss = |left: i32, top: i32| -> f32 {
            let (cw, ch) = cell;
            let mut total = 0.0;
            for board in [theme::SQUARE_LIGHT, theme::SQUARE_DARK] {
                for cy in (0..s).step_by(ch as usize) {
                    for cx in (0..s).step_by(cw as usize) {
                        let mut px: Vec<Rgb> = Vec::with_capacity(8);
                        let mut on: Vec<bool> = Vec::with_capacity(8);
                        for dy in 0..ch {
                            for dx in 0..cw {
                                let c = at(left, top, cx + dx, cy + dy);
                                px.push(c.unwrap_or(board));
                                on.push(c.is_some());
                            }
                        }
                        if on.iter().any(|&o| o) {
                            total += canvas::fit_cell(&px, &on).3;
                        }
                    }
                }
            }
            total
        };
        let reach = |n: i32| -(n - 1)..=(n - 1);
        reach(cell.0)
            .flat_map(|dx| reach(cell.1).map(move |dy| (natural.0 + dx, natural.1 + dy)))
            .filter(|&(l, t)| l >= 0 && t >= 0 && l + width <= s && t + height <= s)
            .map(|(l, t)| {
                let nudge = 0.01 * ((l - natural.0).abs() + (t - natural.1).abs()) as f32;
                ((l, t), loss(l, t) + nudge)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(natural, |(p, _)| p)
    } else {
        natural
    };
    for (y, row) in grid.iter().enumerate() {
        for (x, &key) in row.iter().enumerate() {
            let key = recolour(key, side);
            let Some(i) = sheet::KEYS.iter().position(|&c| c == key) else { continue };
            let [r, g, b, _] = sheet::PALETTE[i].map(|v| v as f32 / 255.0);
            let (x, y) = (pad + left + x as i32, pad + top + y as i32);
            if x >= 0 && y >= 0 && x < w && y < w {
                px[(y * w + x) as usize] = [r, g, b, 1.0];
            }
        }
    }

    // Shadow: the silhouette, box-blurred both ways.
    let mut a: Vec<f32> = px.iter().map(|p| p[3]).collect();
    for horizontal in [true, false] {
        let src = a.clone();
        for y in 0..w {
            for x in 0..w {
                let mut sum = 0.0;
                for k in -blur..=blur {
                    let (xx, yy) = if horizontal { (x + k, y) } else { (x, y + k) };
                    if xx >= 0 && yy >= 0 && xx < w && yy < w {
                        sum += src[(yy * w + xx) as usize];
                    }
                }
                a[(y * w + x) as usize] = sum / (2 * blur + 1) as f32;
            }
        }
    }

    PieceArt {
        sprite: Sprite { w, h: w, px },
        shadow: Mask { w, h: w, a },
        pad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [Kind; 6] = [Kind::Pawn, Kind::Rook, Kind::Knight, Kind::Bishop, Kind::Queen, Kind::King];

    /// Every hand-drawn row is the right width and uses only sheet colours.
    #[test]
    fn hand_drawn_sets_are_well_formed() {
        for kind in KINDS {
            for (set, w, h) in [(small(kind), 7, 8)] {
                assert_eq!(set.len(), h, "{kind:?}");
                for row in set {
                    assert_eq!(row.len(), w, "{kind:?} {row}");
                    assert!(row.bytes().all(|c| c == b'.' || sheet::KEYS.contains(&c)), "{kind:?} {row}");
                }
            }
        }
    }

    /// Pieces stay inside their square in both pixel modes.
    #[test]
    fn pieces_fit_their_squares() {
        for s in [10, 20] {
            for kind in KINDS {
                for side in [Side::White, Side::Black] {
                    for cell in [(1, 2), (2, 4)] {
                    let art = render(kind, side, s, cell);
                    for y in 0..art.sprite.h {
                        for x in 0..art.sprite.w {
                            if art.sprite.px[(y * art.sprite.w + x) as usize][3] > 0.0 {
                                let (sx, sy) = (x - art.pad, y - art.pad);
                                assert!(sx >= 0 && sy >= 0 && sx < s && sy < s, "{kind:?} at {s}px");
                            }
                        }
                    }
                    }
                }
            }
        }
    }
}
