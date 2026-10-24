//! A small RGB pixel canvas. Each terminal cell shows two vertically stacked
//! pixels using the lower half block, so with the usual 1:2 cell shape the
//! pixels come out square. A canvas can also be twice as fine (2x4 pixels per
//! cell), flushed with Unicode 16 octants, for the high-resolution pieces. Everything graphical is drawn here first and
//! flushed to the ratatui buffer once per frame.

use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Color,
};

use crate::octant::OCTANTS;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

pub const fn rgb(r: f32, g: f32, b: f32) -> Rgb {
    Rgb { r, g, b }
}

impl Rgb {
    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        rgb(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
        )
    }

    pub fn scale(self, k: f32) -> Rgb {
        rgb(self.r * k, self.g * k, self.b * k)
    }

    pub fn plus(self, o: Rgb) -> Rgb {
        rgb(self.r + o.r, self.g + o.g, self.b + o.b)
    }

    pub fn to_color(self) -> Color {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        Color::Rgb(c(self.r), c(self.g), c(self.b))
    }
}

/// Premultiplied RGBA image, used for piece art.
pub struct Sprite {
    pub w: i32,
    pub h: i32,
    pub px: Vec<[f32; 4]>,
}

/// Alpha-only image, used for soft shadows.
pub struct Mask {
    pub w: i32,
    pub h: i32,
    pub a: Vec<f32>,
}

#[derive(Clone)]
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    /// Pixels per terminal column: 1 for half blocks, 2 for octants.
    k: i32,
    px: Vec<Rgb>,
    /// Which pixels belong to a sprite, so octant cells can keep piece
    /// silhouettes exact when they have to drop colours.
    sprite: Vec<bool>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        let len = (w.max(0) * h.max(0)) as usize;
        Canvas { w, h, k: 1, px: vec![Rgb::default(); len], sprite: vec![false; len] }
    }

    /// Pixels per terminal column.
    pub fn scale(&self) -> i32 {
        self.k
    }

    /// This canvas made `k` times finer each way, every pixel repeated.
    pub fn upscaled(&self, k: i32) -> Canvas {
        let mut out = Canvas::new(self.w * k, self.h * k);
        out.k = self.k * k;
        for y in 0..out.h {
            for x in 0..out.w {
                out.px[(y * out.w + x) as usize] = self.px[((y / k) * self.w + x / k) as usize];
            }
        }
        out
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.w && y < self.h).then(|| (y * self.w + x) as usize)
    }

    pub fn get(&self, x: i32, y: i32) -> Rgb {
        self.idx(x, y).map_or(Rgb::default(), |i| self.px[i])
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if let Some(i) = self.idx(x, y) {
            self.px[i] = c;
        }
    }

    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if a <= 0.0 {
            return;
        }
        if let Some(i) = self.idx(x, y) {
            self.px[i] = self.px[i].lerp(c, a.min(1.0));
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }

    pub fn blend_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb, a: f32) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.blend(xx, yy, c, a);
            }
        }
    }

    pub fn draw_sprite(&mut self, sp: &Sprite, x: i32, y: i32) {
        for sy in 0..sp.h {
            for sx in 0..sp.w {
                let [r, g, b, a] = sp.px[(sy * sp.w + sx) as usize];
                if a <= 0.0 {
                    continue;
                }
                if let Some(i) = self.idx(x + sx, y + sy) {
                    let d = self.px[i];
                    self.px[i] = rgb(r + d.r * (1.0 - a), g + d.g * (1.0 - a), b + d.b * (1.0 - a));
                    self.sprite[i] = true;
                }
            }
        }
    }

    pub fn draw_mask(&mut self, m: &Mask, x: i32, y: i32, c: Rgb, alpha: f32) {
        for my in 0..m.h {
            for mx in 0..m.w {
                self.blend(x + mx, y + my, c, m.a[(my * m.w + mx) as usize] * alpha);
            }
        }
    }

    /// The single colour a text cell at (col, row) should use as background.
    pub fn cell_bg(&self, col: i32, row: i32) -> Rgb {
        let k = self.k;
        let mut sum = Rgb::default();
        for y in 0..2 * k {
            for x in 0..k {
                sum = sum.plus(self.get(col * k + x, row * 2 * k + y));
            }
        }
        sum.scale(1.0 / (2 * k * k) as f32)
    }

    pub fn flush(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..area.height as i32 {
            for col in 0..area.width as i32 {
                let cell = &mut buf[(area.x + col as u16, area.y + row as u16)];
                if self.k == 1 {
                    half_block(cell, self.get(col, 2 * row), self.get(col, 2 * row + 1));
                } else {
                    let at = |i: usize| (2 * col + i as i32 % 2, 4 * row + i as i32 / 2);
                    let px: [Rgb; 8] = std::array::from_fn(|i| self.get(at(i).0, at(i).1));
                    let sprite: [bool; 8] =
                        std::array::from_fn(|i| self.idx(at(i).0, at(i).1).is_some_and(|j| self.sprite[j]));
                    // Cells without fine detail come out exactly as in the
                    // half-block canvas, so only the pieces change.
                    if px[..4].iter().all(|&p| p == px[0]) && px[4..].iter().all(|&p| p == px[4]) {
                        half_block(cell, px[0], px[4]);
                    } else {
                        octant(cell, &px, &sprite);
                    }
                }
            }
        }
    }
}

fn half_block(cell: &mut Cell, top: Rgb, bot: Rgb) {
    if top.to_color() == bot.to_color() {
        cell.set_symbol(" ").set_bg(bot.to_color());
    } else {
        // The lower half block, not the upper: terminals that take it from
        // the font (macOS Terminal) often draw it a little low. With `▄` that
        // only nudges the split between the two pixels; with `▀` it leaves a
        // sliver of the bottom colour along the top of the cell, which shows
        // as stray lines.
        cell.set_symbol("▄").set_fg(bot.to_color()).set_bg(top.to_color());
    }
}

fn dist(a: Rgb, b: Rgb) -> f32 {
    0.30 * (a.r - b.r).powi(2) + 0.59 * (a.g - b.g).powi(2) + 0.11 * (a.b - b.b).powi(2)
}

/// The best two colours for a block of pixels, which pixels take the second
/// (as a bit mask), and the error. A cell has only two colours, so this
/// tries every pair of the block's own colours and gives each pixel the
/// nearer one. A piece pixel never takes a colour found only on the board
/// (light pieces would vanish into light squares), and a board pixel takes
/// a piece colour only at a cost, so piece silhouettes are kept.
pub fn fit_cell(px: &[Rgb], sprite: &[bool]) -> (Rgb, Rgb, u8, f32) {
    const BOARD_TO_PIECE: f32 = 0.5;
    const PIECE_TO_BOARD: f32 = 10.0;
    // Each distinct colour, and whether it appears on board and on piece pixels.
    let mut colours: Vec<(Rgb, bool, bool)> = Vec::with_capacity(px.len());
    for (&p, &on_piece) in px.iter().zip(sprite) {
        match colours.iter_mut().find(|c| c.0.to_color() == p.to_color()) {
            Some(c) => {
                c.1 |= !on_piece;
                c.2 |= on_piece;
            }
            None => colours.push((p, !on_piece, on_piece)),
        }
    }
    let cost = |i: usize, c: &(Rgb, bool, bool)| {
        let penalty = match (sprite[i], c.1, c.2) {
            (true, _, false) => PIECE_TO_BOARD,
            (false, false, _) => BOARD_TO_PIECE,
            _ => 0.0,
        };
        dist(px[i], c.0) + penalty
    };
    let n = px.len();
    let (mut bg, mut fg, mut best) = (colours[0], colours[0], if colours.len() == 1 { 0.0 } else { f32::MAX });
    for i in 0..colours.len() {
        for j in i + 1..colours.len() {
            let err: f32 = (0..n).map(|p| cost(p, &colours[i]).min(cost(p, &colours[j]))).sum();
            if err < best {
                (bg, fg, best) = (colours[i], colours[j], err);
            }
        }
    }
    let mask = (0..n).filter(|&p| cost(p, &fg) < cost(p, &bg)).fold(0u8, |m, p| m | 1 << p);
    (bg.0, fg.0, mask, best)
}

/// Draws a 2x4 block of pixels (row-major) with one octant character.
fn octant(cell: &mut Cell, px: &[Rgb; 8], sprite: &[bool; 8]) {
    let (bg, fg, mask, _) = fit_cell(px, sprite);
    match mask {
        0 => cell.set_symbol(" ").set_bg(bg.to_color()),
        255 => cell.set_symbol(" ").set_bg(fg.to_color()),
        _ => cell.set_char(OCTANTS[mask as usize]).set_fg(fg.to_color()).set_bg(bg.to_color()),
    };
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic per-pixel noise in 0..1.
pub fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, seed: u32) -> f32 {
    let i = x.floor();
    let a = hash(i as i32, 0, seed);
    let b = hash(i as i32 + 1, 0, seed);
    a + (b - a) * smoothstep(0.0, 1.0, x - i)
}

/// Wood grain in 0..1: streaks running along `along`, wobbling slowly.
pub fn wood(along: f32, across: f32, seed: u32) -> f32 {
    let wobble = value_noise(along * 0.07, seed ^ 0x5bd1) * 2.5;
    let streak = value_noise(across * 0.8 + wobble, seed);
    let fleck = hash(along as i32, across as i32, seed ^ 0x9e37);
    streak * 0.8 + fleck * 0.2
}

/// Nearest xterm-256 colour, for terminals without truecolor.
pub fn to_ansi256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| {
        (0..6).min_by_key(|&i| (LEVELS[i] - v as i32).abs()).unwrap_or(0)
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let dist = |cr: i32, cg: i32, cb: i32| {
        (cr - r as i32).pow(2) + (cg - g as i32).pow(2) + (cb - b as i32).pow(2)
    };
    let cube = dist(LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gi_gray = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + 10 * gi_gray;
    if dist(gv, gv, gv) < cube {
        Color::Indexed(232 + gi_gray as u8)
    } else {
        Color::Indexed(16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8)
    }
}
