//! Draws a frame: background, board and pieces go into the pixel canvas,
//! the panel adds its own pixels, and text is laid on top last.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::time::Instant;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use crate::app::{App, Screen};
use chess_core::{Color as Side, Kind, Piece, Square};
use crate::canvas::{Canvas, Rgb, hash, rgb, smoothstep, to_ansi256, wood};
use crate::layout::{self, BoardGeom, Cells, Fit};
use crate::panel;
use crate::pieces::{self, PieceArt};
use crate::theme::*;

/// A run of text in cell coordinates. Without an explicit background it
/// takes the colour of the pixels underneath.
pub struct Text {
    pub x: i32,
    pub y: i32,
    pub s: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
}

impl Text {
    pub fn new(x: i32, y: i32, s: impl Into<String>, fg: Rgb) -> Self {
        Text { x, y, s: s.into(), fg, bg: None, bold: false }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn on(mut self, bg: Rgb) -> Self {
        self.bg = Some(bg);
        self
    }
}

pub struct Renderer {
    truecolor: bool,
    /// The static scene (background, board, frame) only changes on resize.
    scene_key: Option<(i32, i32, i32, i32, i32)>,
    scene: Canvas,
    /// Piece art by kind, colour, square size and canvas scale.
    art: HashMap<(Kind, Side, i32, i32), PieceArt>,
}

impl Renderer {
    pub fn new(truecolor: bool) -> Self {
        Renderer { truecolor, scene_key: None, scene: Canvas::new(0, 0), art: HashMap::new() }
    }

    pub fn draw(&mut self, frame: &mut Frame, app: &App, now: Instant) {
        let area = frame.area();
        let (w, h) = (area.width as i32, area.height as i32);
        let zoom_in = match layout::fit(w, h) {
            Fit::TooSmall => Some(false),
            Fit::TooBig if !app.play_anyway => Some(true),
            _ => None,
        };
        let layout = match (zoom_in, layout::compute(w, h)) {
            (None, Some(layout)) => layout,
            _ => {
                // The wrong size for the layout: the night sky and how to fix it.
                let mut cv = Canvas::new(w, 2 * h);
                background(&mut cv);
                let buf = frame.buffer_mut();
                cv.flush(buf, area);
                write_text(buf, area, &cv, &panel::zoom_message(zoom_in.unwrap_or(false), w, h));
                return;
            }
        };

        if matches!(app.screen, Screen::Home | Screen::Lobby | Screen::Setup) {
            self.home(frame, app, now);
            return;
        }

        let g = layout.g;
        let key = (w, h, g.s, g.x, g.y);
        if self.scene_key != Some(key) {
            self.scene = Canvas::new(w, 2 * h);
            draw_scene(&mut self.scene, &g);
            self.scene_key = Some(key);
        }

        let mut cv = self.scene.clone();
        let mut text = Vec::new();
        let t = now.saturating_duration_since(app.started).as_secs_f32();
        board_overlays(&mut cv, &mut text, app, &g);
        panel::draw(&mut cv, &mut text, app, layout.panel, now);
        let [top, bottom] = layout.strips;
        let (top_side, bottom_side) = if app.flipped { (Side::White, Side::Black) } else { (Side::Black, Side::White) };
        // Each strip's text sits on the row nearer the board.
        panel::strip(&mut cv, &mut text, app, top_side, top, top.y + top.h - 1, app.hires, now, t);
        panel::strip(&mut cv, &mut text, app, bottom_side, bottom, bottom.y, app.hires, now, t);

        // Pieces and the cursor go on top, on a canvas twice as fine when
        // the terminal can show octants.
        let k = if app.hires { 2 } else { 1 };
        if k > 1 {
            cv = cv.upscaled(k);
        }
        let gk = g.scaled(k);
        if app.promotion.is_none() {
            move_hints(&mut cv, app, &gk);
        }
        self.draw_pieces(&mut cv, app, &gk, now, t);
        let mut cursor_sq = app.cursor;
        if let Some(squares) = app.promotion_squares() {
            self.promotion_picker(&mut cv, app, &gk, squares, t);
            cursor_sq = squares[app.promotion.map_or(0, |p| p.choice)].0;
        }
        if k > 1 {
            for (r, side) in [(top, top_side), (bottom, bottom_side)] {
                self.captured_icons(&mut cv, app, side, r, k);
            }
        }
        if app.screen == Screen::Playing {
            cursor(&mut cv, &gk, cursor_sq, app.flipped, t);
        } else {
            panel::countdown_popup(&mut cv, &mut text, app, &g, now);
        }
        self.finish(frame, &cv, &text);
    }

    /// The menu before a game, in place of the board: the night sky with
    /// the home box and a king on each side of the title.
    fn home(&mut self, frame: &mut Frame, app: &App, now: Instant) {
        let area = frame.area();
        let (w, h) = (area.width as i32, area.height as i32);
        let t = now.saturating_duration_since(app.started).as_secs_f32();
        let mut cv = Canvas::new(w, 2 * h);
        background(&mut cv);
        let mut text = Vec::new();
        panel::home(&mut cv, &mut text, app, w, h, now, t);
        let k = if app.hires { 2 } else { 1 };
        if k > 1 {
            cv = cv.upscaled(k);
        }
        let g = BoardGeom { s: layout::SQUARE * k, f: 0, slab: 0, shadow: 0, x: 0, y: 0 };
        let (x, y) = panel::home_origin(w, h);
        for (dx, color) in panel::HOME_TILES.into_iter().zip([Side::White, Side::Black]) {
            self.piece(&mut cv, &g, Piece { color, kind: Kind::King }, (x + dx) * k, 2 * (y + panel::HOME_TILE_ROW) * k, 0.0);
        }
        self.finish(frame, &cv, &text);
    }

    /// Flushes the canvas and text to the terminal, mapping colours to
    /// xterm-256 when there is no truecolor.
    fn finish(&self, frame: &mut Frame, cv: &Canvas, text: &[Text]) {
        let area = frame.area();
        let buf = frame.buffer_mut();
        cv.flush(buf, area);
        write_text(buf, area, cv, text);
        if !self.truecolor {
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    let cell = &mut buf[(x, y)];
                    let (fg, bg) = (to_ansi256(cell.fg), to_ansi256(cell.bg));
                    cell.set_fg(fg).set_bg(bg);
                }
            }
        }
    }

    /// Draws every piece, measuring with `g` on canvas `cv`.
    fn draw_pieces(&mut self, cv: &mut Canvas, app: &App, g: &BoardGeom, now: Instant, t: f32) {
        let s = g.s;
        let flipped = app.flipped;

        // Back rows first, so taller pieces in front overlap the ones behind.
        let slide = app.slide.as_ref();
        for row in 0..8 {
            for col in 0..8 {
                let sq = BoardGeom::square_at_screen(col, row, flipped);
                if Some(sq) == app.lifted() {
                    continue;
                }
                let mut piece = app.piece_at(sq);
                if let Some(sl) = slide {
                    // Movers are drawn in flight below; the captured piece
                    // stays put until the mover arrives.
                    if sl.movers.iter().any(|&(_, _, to)| to == sq) {
                        piece = None;
                    }
                    if let Some((p, at)) = sl.captured
                        && at == sq
                    {
                        piece = Some(p);
                    }
                }
                if let Some(p) = piece {
                    let (x, y) = g.square_px(sq, flipped);
                    self.piece(cv, g, p, x, y, 0.0);
                }
            }
        }

        if let Some(sq) = app.lifted()
            && let Some(p) = app.piece_at(sq)
        {
            let hover = s as f32 * (0.12 + 0.025 * (t * 4.0).sin());
            // A dragged piece hangs centred under the mouse.
            let (x, y) = match app.drag {
                Some(d) if d.moved => {
                    let k = cv.scale();
                    (d.at.0 * k + k / 2 - s / 2, (2 * d.at.1 + 1) * k - s / 2)
                }
                _ => g.square_px(sq, flipped),
            };
            self.piece(cv, g, p, x, y, hover.max(1.0));
        }

        if let Some(sl) = slide {
            let p = sl.progress(now);
            for &(piece, from, to) in &sl.movers {
                let (x0, y0) = g.square_px(from, flipped);
                let (x1, y1) = g.square_px(to, flipped);
                let x = x0 as f32 + (x1 - x0) as f32 * p;
                let y = y0 as f32 + (y1 - y0) as f32 * p;
                let arc = (PI * p).sin() * s as f32 * 0.18;
                self.piece(cv, g, piece, x.round() as i32, y.round() as i32, arc);
            }
        }
    }

    /// The choice of piece for a promotion: the board dims and a column of
    /// raised tiles runs from the promotion square toward the middle.
    fn promotion_picker(&mut self, cv: &mut Canvas, app: &App, g: &BoardGeom, squares: [(Square, Kind); 4], t: f32) {
        let s = g.s;
        let k = cv.scale();
        cv.blend_rect(g.x + g.f, g.y + g.f, 8 * s, 8 * s, SHADOW, 0.55);
        let color = app.to_move();
        let choice = app.promotion.map_or(0, |p| p.choice);
        for (i, &(sq, kind)) in squares.iter().enumerate() {
            let (x, y) = g.square_px(sq, app.flipped);
            let chosen = i == choice;
            // A tile lit from the top left, like the panel's bevel.
            cv.fill_rect(x, y, s, s, PANEL_OUTLINE);
            cv.fill_rect(x + k, y + k, s - 2 * k, s - 2 * k, if chosen { SELECTED.scale(0.55) } else { PANEL_DARK });
            cv.fill_rect(x + k, y + k, s - 3 * k, s - 3 * k, if chosen { SELECTED } else { PANEL_LIGHT });
            cv.fill_rect(x + 2 * k, y + 2 * k, s - 4 * k, s - 4 * k, if chosen { SELECTED.scale(0.8) } else { PANEL });
            let lift = if chosen { s as f32 * (0.08 + 0.02 * (t * 4.0).sin()) } else { 0.0 };
            self.piece(cv, g, Piece { color, kind }, x, y, lift);
        }
    }

    /// The pieces `side` has captured, as small pixel art in its strip, on a
    /// canvas `k` times finer than the board.
    fn captured_icons(&mut self, cv: &mut Canvas, app: &App, side: Side, r: Cells, k: i32) {
        // A strip is two rows: 4k pixels tall, so icons are drawn as if on
        // 4k-pixel squares, which picks the small 7x8 set.
        let s = 4 * k;
        for (i, p) in app.captured_by(side).into_iter().enumerate() {
            let art = self
                .art
                .entry((p.kind, p.color, s, k))
                .or_insert_with(|| pieces::render(p.kind, p.color, s, (k, 2 * k)));
            let x = (panel::captured_x(r) + i as i32 * panel::CAPTURED_COLS) * k;
            // Each sits on a tiny tile of board colour, alternating like a
            // rank of squares, so dark pieces read against the dark strip.
            let tile = if i % 2 == 0 { SQUARE_LIGHT } else { SQUARE_DARK };
            cv.fill_rect(x, 2 * r.y * k, panel::CAPTURED_COLS * k, s, tile);
            cv.draw_sprite(&art.sprite, x - art.pad, 2 * r.y * k - art.pad);
        }
    }

    /// Draws a piece whose square starts at (x, y), raised by `lift` pixels.
    fn piece(&mut self, cv: &mut Canvas, g: &BoardGeom, p: Piece, x: i32, y: i32, lift: f32) {
        let s = g.s;
        let k = cv.scale();
        let art = self
            .art
            .entry((p.kind, p.color, s, k))
            .or_insert_with(|| pieces::render(p.kind, p.color, s, (k, 2 * k)));
        let sf = s as f32;

        // Contact shadow spreads and fades as the piece rises.
        let spread = 1.0 + 2.0 * lift / sf;
        contact_shadow(cv, x as f32 + sf * 0.53, y as f32 + sf * 0.915, sf * 0.33 * spread, (sf * 0.065 * spread).max(1.0), 0.45 / spread);
        // Cast shadow: the silhouette, falling away from the light.
        let dx = (sf * 0.07 + lift * 0.6).round() as i32;
        let dy = (sf * 0.03 + lift * 0.3).round() as i32;
        cv.draw_mask(&art.shadow, x - art.pad + dx, y - art.pad + dy, SHADOW, 0.32);
        cv.draw_sprite(&art.sprite, x - art.pad, y - art.pad - lift.round() as i32);
    }
}

/// Last-move and selection tints, a red glow under a king in check, and the
/// coordinate labels.
fn board_overlays(cv: &mut Canvas, text: &mut Vec<Text>, app: &App, g: &BoardGeom) {
    let s = g.s;
    let pos = app.game.position();
    if let Some((a, b)) = app.last_move {
        for sq in [a, b] {
            let (x, y) = g.square_px(sq, app.flipped);
            cv.blend_rect(x, y, s, s, LAST_MOVE, 0.38);
        }
    }
    if pos.in_check()
        && let Some(king) = pos.king(pos.turn())
    {
        let (x, y) = g.square_px(king, app.flipped);
        let c = (s - 1) as f32 / 2.0;
        for j in 0..s {
            for i in 0..s {
                let d = ((i as f32 - c).powi(2) + (j as f32 - c).powi(2)).sqrt() / (s as f32 * 0.72);
                cv.blend(x + i, y + j, CHECK, 0.9 * (1.0 - smoothstep(0.1, 1.0, d)));
            }
        }
    }
    if let Some(sq) = app.lifted() {
        let (x, y) = g.square_px(sq, app.flipped);
        cv.blend_rect(x, y, s, s, SELECTED, 0.40);
        ring(cv, x, y, s, s, SELECTED, 0.9);
    }
    labels(text, g, app.flipped);
}

/// Marks where the selected piece can go. Drawn with the pieces, on the
/// finer canvas when there is one, so the dots come out round.
fn move_hints(cv: &mut Canvas, app: &App, g: &BoardGeom) {
    let pos = app.game.position();
    for mv in app.hints() {
        let (x, y) = g.square_px(mv.to, app.flipped);
        if pos.captured(mv).is_some() {
            capture_hint(cv, x, y, g.s);
        } else {
            move_hint(cv, x, y, g.s);
        }
    }
}

/// A dot in the middle of a square the selected piece can move to.
fn move_hint(cv: &mut Canvas, x: i32, y: i32, s: i32) {
    let c = s as f32 / 2.0;
    let r = s as f32 * 0.19;
    for j in 0..s {
        for i in 0..s {
            if (i as f32 + 0.5 - c).hypot(j as f32 + 0.5 - c) < r {
                cv.blend(x + i, y + j, MOVE_HINT, 0.55);
            }
        }
    }
}

/// Corner wedges on a square whose piece can be captured, so they show
/// around the piece.
fn capture_hint(cv: &mut Canvas, x: i32, y: i32, s: i32) {
    let n = (s * 3 / 10).max(2);
    for j in 0..n {
        for i in 0..n - j {
            for (cx, cy) in [(x + i, y + j), (x + s - 1 - i, y + j), (x + i, y + s - 1 - j), (x + s - 1 - i, y + s - 1 - j)] {
                cv.blend(cx, cy, MOVE_HINT, 0.55);
            }
        }
    }
}

fn write_text(buf: &mut Buffer, area: Rect, cv: &Canvas, items: &[Text]) {
    for it in items {
        if it.y < 0 || it.y >= area.height as i32 {
            continue;
        }
        for (i, ch) in it.s.chars().enumerate() {
            let x = it.x + i as i32;
            if x < 0 || x >= area.width as i32 {
                continue;
            }
            let bg = it.bg.unwrap_or_else(|| cv.cell_bg(x, it.y));
            let mut style = Style::new().fg(it.fg.to_color()).bg(bg.to_color());
            if it.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            buf[(area.x + x as u16, area.y + it.y as u16)].set_char(ch).set_style(style);
        }
    }
}

fn ring(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, c: Rgb, a: f32) {
    for i in 0..w {
        cv.blend(x + i, y, c, a);
        cv.blend(x + i, y + h - 1, c, a);
    }
    for j in 1..h - 1 {
        cv.blend(x, y + j, c, a);
        cv.blend(x + w - 1, y + j, c, a);
    }
}

fn contact_shadow(cv: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, alpha: f32) {
    for y in (cy - ry - 1.0) as i32..=(cy + ry + 1.0) as i32 {
        for x in (cx - rx - 1.0) as i32..=(cx + rx + 1.0) as i32 {
            let d = (((x as f32 + 0.5 - cx) / rx).powi(2) + ((y as f32 + 0.5 - cy) / ry).powi(2)).sqrt();
            cv.blend(x, y, SHADOW, alpha * (1.0 - smoothstep(0.35, 1.0, d)));
        }
    }
}

/// Pulsing corner brackets, like a cursor in an old console game.
fn cursor(cv: &mut Canvas, g: &BoardGeom, sq: Square, flipped: bool, t: f32) {
    let (x, y) = g.square_px(sq, flipped);
    let s = g.s;
    let pulse = 0.72 + 0.28 * (t * 5.0).sin();
    cv.blend_rect(x, y, s, s, CURSOR, 0.10);
    let len = (s / 4).max(2);
    let th = (s / 10).max(1);
    for (cx, cy, dx, dy) in [(x, y, 1, 1), (x + s - 1, y, -1, 1), (x, y + s - 1, 1, -1), (x + s - 1, y + s - 1, -1, -1)] {
        for i in 0..len {
            for j in 0..th {
                cv.blend(cx + dx * i, cy + dy * j, CURSOR, pulse);
                if i >= th {
                    cv.blend(cx + dx * j, cy + dy * i, CURSOR, pulse);
                }
            }
        }
    }
}

/// How far in from the outer edge the coordinate labels sit. Chosen so the
/// label row on the bottom frame never lands on the inlay line.
fn label_depth(f: i32) -> i32 {
    if f >= 6 { (f - 2) / 2 } else { f / 2 }
}

fn labels(text: &mut Vec<Text>, g: &BoardGeom, flipped: bool) {
    let d = label_depth(g.f);
    let col = g.x + d;
    let bottom_row = (g.y + g.outer() - 1 - d) / 2;
    for i in 0..8 {
        let sq = BoardGeom::square_at_screen(i, i, flipped);
        let rank = (b'1' + chess_core::rank_of(sq) as u8) as char;
        let file = (b'a' + chess_core::file_of(sq) as u8) as char;
        let row = (g.y + g.f + i * g.s + g.s / 2 - 1) / 2;
        text.push(Text::new(col, row, rank.to_string(), LABEL).bold());
        text.push(Text::new(g.x + g.f + i * g.s + (g.s - 1) / 2, bottom_row, file.to_string(), LABEL).bold());
    }
}

fn draw_scene(cv: &mut Canvas, g: &BoardGeom) {
    background(cv);
    board_shadow(cv, g);
    slab(cv, g);
    frame(cv, g);
    squares(cv, g);
    inner_shadow(cv, g);
}

/// Night-sky gradient with a vignette, faint stars and CRT scanlines.
fn background(cv: &mut Canvas) {
    for y in 0..cv.h {
        let ty = y as f32 / cv.h.max(1) as f32;
        let base = BG_TOP.lerp(BG_BOTTOM, ty);
        let scan = if y % 2 == 1 { 0.88 } else { 1.0 };
        for x in 0..cv.w {
            let nx = (x as f32 / cv.w.max(1) as f32 - 0.5) * 2.0;
            let ny = (ty - 0.5) * 2.0;
            let vignette = 1.0 - 0.45 * (nx * nx * 0.6 + ny * ny * 0.4).min(1.0);
            let mut c = base.scale(vignette * scan * (0.98 + 0.04 * hash(x, y, 1)));
            let star = hash(x, y, 7);
            if star > 0.9965 {
                c = c.lerp(STAR, (star - 0.9965) / 0.0035 * 0.5);
            }
            cv.set(x, y, c);
        }
    }
}

fn board_shadow(cv: &mut Canvas, g: &BoardGeom) {
    let off = g.shadow / 2;
    let blur = g.shadow as f32 + 1.0;
    let (x0, y0) = (g.x + off, g.y + off);
    let (x1, y1) = (x0 + g.outer() - 1, y0 + g.outer() + g.slab - 1);
    let b = blur as i32;
    for y in y0 - b..=y1 + b {
        for x in x0 - b..=x1 + b {
            let dx = (x0 - x).max(x - x1).max(0) as f32;
            let dy = (y0 - y).max(y - y1).max(0) as f32;
            let d = (dx * dx + dy * dy).sqrt();
            cv.blend(x, y, SHADOW, 0.75 * (1.0 - smoothstep(0.0, blur, d)));
        }
    }
}

/// The board's front edge, seen just below the frame.
fn slab(cv: &mut Canvas, g: &BoardGeom) {
    let top = g.y + g.outer();
    let w = g.outer();
    for i in 0..g.slab {
        let t = i as f32 / (g.slab - 1).max(1) as f32;
        let y = top + i;
        for x in g.x..g.x + w {
            let mut c = SLAB_TOP.lerp(SLAB_BOTTOM, t).scale(0.85 + 0.3 * wood(x as f32, y as f32 * 3.0, 11));
            if i == 0 {
                c = c.scale(1.35);
            }
            if x == g.x {
                c = c.scale(0.8);
            } else if x == g.x + w - 1 {
                c = c.scale(0.6);
            }
            cv.set(x, y, c);
        }
    }
}

/// A raised walnut frame: grain running along each side, mitred corners,
/// a bevel lit from the top left, and a brass inlay on larger boards.
fn frame(cv: &mut Canvas, g: &BoardGeom) {
    let (o, f, inner) = (g.outer(), g.f, 8 * g.s);
    for ly in 0..o {
        for lx in 0..o {
            if lx >= f && lx < f + inner && ly >= f && ly < f + inner {
                continue;
            }
            let (dl, dr, dt, db) = (lx, o - 1 - lx, ly, o - 1 - ly);
            let (dh, dv) = (dt.min(db), dl.min(dr));
            let horizontal = dh <= dv;
            let d = dh.min(dv);
            let (x, y) = (g.x + lx, g.y + ly);

            let grain = if horizontal { wood(x as f32, y as f32, 3) } else { wood(y as f32, x as f32, 5) };
            let faces_light = if horizontal { dt <= db } else { dl <= dr };
            let sign = if faces_light { 1.0 } else { -1.0 };
            let shade = if d == 0 {
                1.0 + 0.40 * sign
            } else if d == 1 && f >= 4 {
                1.0 + 0.18 * sign
            } else if d == f - 1 {
                1.0 - 0.32 * sign
            } else {
                // Gently rounded moulding between the two edges.
                1.0 + 0.10 * sign * (1.0 - 2.0 * d as f32 / (f - 1) as f32)
            };

            let c = if dh == dv {
                FRAME.scale((0.78 + 0.38 * grain) * shade * 0.7)
            } else if f >= 6 && d == f - 2 {
                INLAY.scale(0.85 + 0.25 * grain + if faces_light { 0.1 } else { -0.1 })
            } else {
                FRAME.scale((0.78 + 0.38 * grain) * shade)
            };
            cv.set(x, y, c);
        }
    }
}

/// Squares with fine grain, alternating direction like parquet.
fn squares(cv: &mut Canvas, g: &BoardGeom) {
    for sq in 0..64 {
        let (x0, y0) = g.square_px(sq, false);
        let parity = (chess_core::file_of(sq) + chess_core::rank_of(sq)) % 2;
        let base = if parity == 1 { SQUARE_LIGHT } else { SQUARE_DARK };
        let seed = 20 + sq as u32;
        for y in y0..y0 + g.s {
            for x in x0..x0 + g.s {
                let grain = if parity == 1 {
                    wood(x as f32, y as f32 * 1.5, seed)
                } else {
                    wood(y as f32, x as f32 * 1.5, seed)
                };
                cv.set(x, y, base.scale(0.93 + 0.11 * grain));
            }
        }
    }
}

/// The frame casts a shadow onto the top and left of the playing surface.
fn inner_shadow(cv: &mut Canvas, g: &BoardGeom) {
    let (x0, y0, n) = (g.x + g.f, g.y + g.f, 8 * g.s);
    let w = (g.s / 6).max(1);
    for i in 0..w {
        let a = 0.42 * (1.0 - i as f32 / w as f32);
        for k in 0..n {
            cv.blend(x0 + k, y0 + i, SHADOW, a);
            if k >= i {
                cv.blend(x0 + i, y0 + k, SHADOW, a);
            }
        }
    }
    for k in 0..n {
        cv.blend(x0 + k, y0 + n - 1, rgb(1.0, 1.0, 1.0), 0.07);
        cv.blend(x0 + n - 1, y0 + k, rgb(1.0, 1.0, 1.0), 0.07);
    }
}
