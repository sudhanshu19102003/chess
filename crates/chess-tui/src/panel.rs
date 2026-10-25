//! The panel beside the board (move history, key help), the clock strips
//! above and below the board, and the zoom messages.

use std::time::Instant;

use chess_core::Color as Side;

use chess_net::Via;
use chess_verify::Signal;
use chess_net::connect::Stage;

use crate::app::{self, App, EndChoice, EngineStatus, Lobby, Mode, MoveRecord, Pick, Screen, Setting};
use crate::canvas::{Canvas, Rgb, hash, rgb};
use crate::clock;
use crate::engine;
use crate::layout::{self, BoardGeom, Cells};
use crate::render::Text;
use crate::theme::*;


/// Rows of the box about a network game.
const ONLINE_H: i32 = 8;
/// Rows of the box about a game against the computer.
const VERSUS_H: i32 = 5;
/// Rows of the box for a typed move: the input and the suggestions.
const INPUT_H: i32 = 5 + app::SUGGESTIONS as i32;

/// Rows of the draw and resign buttons and the line under them.
const ACTIONS_H: i32 = 4;

/// Where the panel's parts go, top to bottom: the history, then the
/// typed-move box, the draw and resign buttons and the network box when
/// they show, then the key help. Each is the row it starts on.
struct Sections {
    history_h: i32,
    input: Option<i32>,
    actions: Option<i32>,
    online: Option<i32>,
    versus: Option<i32>,
    help: i32,
}

fn sections(app: &App, r: Cells, help_rows: i32) -> Sections {
    let mut y = r.y + r.h - help_rows;
    let mut take = |shown: bool, h: i32| {
        shown.then(|| {
            y -= h + 1;
            y + 1
        })
    };
    let online = take(app.online.is_some(), ONLINE_H);
    let versus = take(app.versus.is_some(), VERSUS_H);
    let actions = take(!app.over(), ACTIONS_H);
    let input = take(app.input.is_some(), INPUT_H);
    let history_h = y - r.y;
    Sections { history_h, input, actions, online, versus, help: r.y + r.h - help_rows }
}

pub fn draw(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, r: Cells, now: Instant) {
    let help = help_lines(app, r.w - 2);
    let at = sections(app, r, help.len() as i32);
    history(cv, out, app, r.x, r.y, r.w, at.history_h);
    if let Some(y) = at.input {
        move_input(cv, out, app, r.x, y, r.w, INPUT_H);
    }
    if let Some(y) = at.actions {
        actions(cv, out, app, r.x, y, r.w, now);
    }
    if let Some(y) = at.online {
        online(cv, out, app, r.x, y, r.w, ONLINE_H);
    }
    if let Some(y) = at.versus {
        versus(cv, out, app, r.x, y, r.w, VERSUS_H, now);
    }
    for (i, line) in help.iter().enumerate() {
        put_spans(out, r.x + 1, at.help + i as i32, line);
    }
}

/// The draw and resign buttons: their columns and width.
fn action_buttons(x: i32, w: i32) -> [(Signal, i32, i32); 2] {
    let bw = (w - 2) / 2;
    [(Signal::OfferDraw, x, bw), (Signal::Resign, x + w - bw, bw)]
}

/// Which of the draw and resign buttons the cell (`col`, `row`) is on:
/// `OfferDraw` for the draw button whatever it does, or `Resign`.
pub fn action_hit(app: &App, r: Cells, col: i32, row: i32) -> Option<Signal> {
    let y = sections(app, r, help_lines(app, r.w - 2).len() as i32).actions?;
    action_buttons(r.x, r.w)
        .into_iter()
        .find(|&(_, bx, bw)| (bx..bx + bw).contains(&col) && (y..y + 3).contains(&row))
        .map(|(kind, _, _)| kind)
}

/// The draw and resign buttons, and a line about any offer or
/// resignation under way.
fn actions(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, w: i32, now: Instant) {
    let armed = app.resign_armed(now);
    let pending = app.pending();
    let actor = app.actor();
    let draw = app.draw_action();
    for (kind, bx, bw) in action_buttons(x, w) {
        let (label, colour, glow) = if kind == Signal::Resign {
            match (armed, pending) {
                (_, Some(Signal::Resign)) => ("RESIGNING…", ALARM_TOP, Some(ALARM_BOTTOM)),
                (true, _) => ("CONFIRM RESIGN", ALARM_TOP, Some(ALARM_BOTTOM)),
                _ if pending.is_some() => ("RESIGN", TEXT_DIM, None),
                _ => ("RESIGN", TEXT, None),
            }
        } else {
            match (draw, pending, app.draw_offer) {
                (_, Some(Signal::AcceptDraw), _) => ("ACCEPTING…", AMBER_TOP, Some(AMBER)),
                (Some(Signal::AcceptDraw), _, _) => ("ACCEPT DRAW", AMBER_TOP, Some(AMBER)),
                (Some(_), _, _) => ("OFFER DRAW", TEXT, None),
                (None, _, Some(by)) if by == actor => ("DRAW OFFERED", TEXT_DIM, None),
                _ => ("OFFER DRAW", TEXT_DIM, None),
            }
        };
        bevel_box(cv, bx, y, bw, 3, glow);
        centred(out, bx, bw, y + 1, label, colour);
    }

    let line = match (pending, app.draw_offer) {
        (Some(Signal::Resign), _) => "you resign as soon as their move arrives".to_string(),
        (Some(_), _) => "you accept as soon as their move arrives".to_string(),
        _ if armed => "press X or click again to resign".to_string(),
        (None, Some(by)) if by != actor => format!("{} offers a draw: D to accept", by.name()),
        (None, Some(_)) if app.online.is_some() => "they may accept on their turn".to_string(),
        (None, Some(by)) => format!("{} may accept on their turn", by.opponent().name()),
        _ => String::new(),
    };
    let colour = if armed || pending == Some(Signal::Resign) { ALARM_TOP } else { AMBER };
    centred(out, x, w, y + 3, &line, colour);
}

/// A raised panel: outline, a two-tone bevel, a faint sheen and a drop
/// shadow. Its first and last rows and two outer columns are border. Works
/// on a canvas of any scale, so it can also go on top of the pieces.
fn bevel_box(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, glow: Option<Rgb>) {
    let k = cv.scale();
    let (px, py, pw, ph) = (x * k, 2 * y * k, w * k, 2 * h * k);
    cv.blend_rect(px + 2 * k, py + 2 * k, pw, ph, SHADOW, 0.5);
    if let Some(gc) = glow {
        cv.blend_rect(px - k, py - k, pw + 2 * k, k, gc, 0.35);
        cv.blend_rect(px - k, py + ph, pw + 2 * k, k, gc, 0.35);
        cv.blend_rect(px - k, py, k, ph, gc, 0.35);
        cv.blend_rect(px + pw, py, k, ph, gc, 0.35);
    }
    let (light, dark) = match glow {
        Some(gc) => (gc, gc.scale(0.45)),
        None => (PANEL_LIGHT, PANEL_DARK),
    };
    cv.fill_rect(px, py, pw, ph, PANEL_OUTLINE);
    cv.fill_rect(px + k, py + k, pw - 2 * k, ph - 2 * k, dark);
    cv.fill_rect(px + k, py + k, pw - 3 * k, ph - 3 * k, light);
    cv.fill_rect(px + 2 * k, py + 2 * k, pw - 4 * k, ph - 4 * k, PANEL);
    let inner_h = (ph - 4 * k).max(1);
    for j in 0..ph - 4 * k {
        let a = 0.05 * (1.0 - j as f32 / inner_h as f32);
        cv.blend_rect(px + 2 * k, py + 2 * k + j, pw - 4 * k, 1, rgb(1.0, 1.0, 1.0), a);
    }
}

fn centred(out: &mut Vec<Text>, x: i32, w: i32, row: i32, s: &str, c: Rgb) {
    out.push(Text::new(x + (w - s.chars().count() as i32) / 2, row, s, c).bold());
}

fn centred_spans(out: &mut Vec<Text>, x: i32, w: i32, row: i32, spans: &Spans) {
    let len: i32 = spans.iter().map(|(s, _, _)| s.chars().count() as i32).sum();
    put_spans(out, x + (w - len) / 2, row, spans);
}

fn time_control(app: &App) -> String {
    let tc = app.clock.control();
    format!("{} {}", tc.name(), tc.label)
}

/// The time control with arrows to change it.
fn time_spans(app: &App) -> Spans {
    vec![
        ("TIME  ".into(), TEXT_DIM, false),
        ("◀ ".into(), AMBER, true),
        (time_control(app), TEXT, true),
        (" ▶".into(), AMBER, true),
    ]
}

/// Row of the time control on the home screen and in the lobby, below the
/// three buttons.
const HOME_TIME_ROW: i32 = 22;
/// A menu button: its column from the home box's left edge, its width, and
/// the row of the first one. An emoji sits before its label, so it is
/// as big as the text.
const BUTTON_W: i32 = 32;
const BUTTON_X: i32 = (HOME_W - BUTTON_W) / 2;
const BUTTONS_ROW: i32 = 10;

fn mode_glyph(mode: Mode) -> char {
    match mode {
        Mode::Offline => '👥',
        Mode::PeerToPeer => '🌐',
        Mode::Computer => '🤖',
    }
}

fn pick_glyph(pick: Pick) -> char {
    match pick {
        Pick::Host => '🌐',
        Pick::HostLan => '📡',
        Pick::Join => '🔑',
    }
}
/// The game code's box and the copy button beside it, in the lobby: from
/// the home box's left edge, and their width.
const CODE_BOX: (i32, i32) = (10, 28);
const COPY_BUTTON: (i32, i32) = (40, 10);
const CODE_ROW: i32 = 10;

/// What a click on the home screen or in the lobby landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeHit {
    Mode(Mode),
    /// The time control: true for the next one, false for the previous.
    Time(bool),
    /// A lobby button.
    Pick(Pick),
    /// The game code or the copy button.
    Copy,
    /// A VS COMPUTER setting: true for the next value, false for the previous.
    Setting(Setting, bool),
    /// The VS COMPUTER play button.
    Play,
}

/// What the cell (`col`, `row`) of a `w` x `h` window is on the home screen
/// or in the lobby, with the same geometry as `home` draws.
pub fn home_hit(app: &App, w: i32, h: i32, col: i32, row: i32) -> Option<HomeHit> {
    let (x, y) = home_origin(w, h);
    let inside = |bx: i32, by: i32, bw: i32, bh: i32| (bx..bx + bw).contains(&col) && (by..by + bh).contains(&row);
    let button = |i: i32| inside(x + BUTTON_X, y + BUTTONS_ROW + 4 * i, BUTTON_W, 3);
    // The arrows and the name between them: the left half goes back.
    let time = |trow: i32| {
        let len: i32 = time_spans(app).iter().map(|(s, _, _)| s.chars().count() as i32).sum();
        let start = x + (HOME_W - len) / 2;
        (row == trow && (start..start + len).contains(&col)).then(|| HomeHit::Time(col >= start + len / 2))
    };
    match (app.screen, &app.lobby) {
        (Screen::Home, _) => (0..Mode::ALL.len())
            .find(|&i| button(i as i32))
            .map(|i| HomeHit::Mode(Mode::ALL[i]))
            .or_else(|| time(y + HOME_TIME_ROW)),
        (Screen::Lobby, Lobby::Choose { pick }) => (0..Pick::ALL.len())
            .find(|&i| button(i as i32))
            .map(|i| HomeHit::Pick(Pick::ALL[i]))
            .or_else(|| if *pick == Pick::Join { None } else { time(y + HOME_TIME_ROW) }),
        (Screen::Setup, _) => SETUP_ROWS
            .into_iter()
            .find_map(|(setting, srow)| {
                let len: i32 = setting_spans(app, setting).iter().map(|(s, _, _)| s.chars().count() as i32).sum();
                let start = x + (HOME_W - len) / 2;
                (row == y + srow && (start..start + len).contains(&col)).then(|| HomeHit::Setting(setting, col >= start + len / 2))
            })
            .or_else(|| inside(x + (HOME_W - PLAY_W) / 2, y + PLAY_ROW, PLAY_W, 3).then_some(HomeHit::Play)),
        (Screen::Lobby, Lobby::Connecting { code: Some(_), .. }) => {
            let on_box = inside(x + CODE_BOX.0, y + CODE_ROW, CODE_BOX.1, 3);
            (on_box || inside(x + COPY_BUTTON.0, y + CODE_ROW, COPY_BUTTON.1, 3)).then_some(HomeHit::Copy)
        }
        _ => None,
    }
}

/// The home screen's box, `HOME_W` x `HOME_H` cells, centred in a `w` x `h`
/// window. Its top-left cell.
pub const HOME_W: i32 = 60;
pub const HOME_H: i32 = 30;
pub fn home_origin(w: i32, h: i32) -> (i32, i32) {
    ((w - HOME_W) / 2, (h - HOME_H) / 2)
}
/// Columns from the box's left edge to each of the two piece tiles beside the
/// title, which the renderer fills.
pub const HOME_TILES: [i32; 2] = [6, HOME_W - 6 - layout::SQUARE];
/// The row the tiles start on, from the top of the box.
pub const HOME_TILE_ROW: i32 = 3;

/// A gold inlaid frame inside the home box's bevel: a thin line, a bright
/// stud at each corner and a diamond in the middle of each side.
fn home_frame(cv: &mut Canvas, x: i32, y: i32) {
    let k = cv.scale();
    let (px, py, pw, ph) = (x * k, 2 * y * k, HOME_W * k, 2 * HOME_H * k);
    let (gold, dim) = (AMBER.scale(0.75), AMBER.scale(0.35));
    let inset = 3 * k;
    let (l, t, w, h) = (px + inset, py + inset, pw - 2 * inset, ph - 2 * inset);
    cv.fill_rect(l, t, w, k, gold);
    cv.fill_rect(l, t + h - k, w, k, gold);
    cv.fill_rect(l, t, k, h, gold);
    cv.fill_rect(l + w - k, t, k, h, gold);
    // A fainter shade just inside, so the line looks inlaid.
    cv.fill_rect(l + k, t + k, w - 2 * k, k, dim);
    cv.fill_rect(l + k, t + h - 2 * k, w - 2 * k, k, dim);
    cv.fill_rect(l + k, t + k, k, h - 2 * k, dim);
    cv.fill_rect(l + w - 2 * k, t + k, k, h - 2 * k, dim);
    for (cx, cy) in [(l - k, t - k), (l + w - 2 * k, t - k), (l - k, t + h - 2 * k), (l + w - 2 * k, t + h - 2 * k)] {
        cv.fill_rect(cx, cy, 3 * k, 3 * k, AMBER_TOP);
        cv.fill_rect(cx + k, cy + k, k, k, PANEL_OUTLINE);
    }
    for cx in [l - k, l + w - 2 * k] {
        let cy = py + ph / 2;
        for (dx, dy, w, h) in [(1, -2, 1, 5), (0, -1, 3, 3)] {
            cv.fill_rect(cx + dx * k, cy + dy * k, w * k, h * k, AMBER_TOP);
        }
    }
}

/// The home screen before a game: a title between two tiles, one button per
/// mode (or the peer-to-peer lobby), the time control and the keys. Drawn on
/// the base canvas.
pub fn home(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, w: i32, h: i32, now: Instant, t: f32) {
    let (x, y) = home_origin(w, h);
    bevel_box(cv, x, y, HOME_W, HOME_H, None);
    home_frame(cv, x, y);
    for (i, dx) in HOME_TILES.into_iter().enumerate() {
        let (tx, ty, s) = (x + dx, 2 * (y + HOME_TILE_ROW), layout::SQUARE);
        cv.blend_rect(tx + 1, ty + 1, s, s, SHADOW, 0.6);
        cv.fill_rect(tx - 1, ty - 1, s + 2, s + 2, PANEL_OUTLINE);
        cv.fill_rect(tx, ty, s, s, if i == 0 { SQUARE_DARK } else { SQUARE_LIGHT });
    }
    centred(out, x, HOME_W, y + 3, "CHESS", AMBER);
    if app.screen == Screen::Lobby {
        centred(out, x, HOME_W, y + 5, "play a friend", TEXT_DIM);
        return lobby(cv, out, app, x, y, now, t);
    }
    if app.screen == Screen::Setup {
        centred(out, x, HOME_W, y + 5, "play the computer", TEXT_DIM);
        return setup(cv, out, app, x, y, t);
    }
    centred(out, x, HOME_W, y + 5, "choose a game", TEXT_DIM);

    // The chosen button glows brighter and dimmer, like an arcade "press start".
    let pulse = 0.75 + 0.25 * (t * 3.0).sin();
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let by = y + BUTTONS_ROW + 4 * i as i32;
        button(cv, out, x, by, mode.name(), mode_glyph(mode), mode == app.mode, pulse);
    }
    centred_spans(out, x, HOME_W, y + HOME_TIME_ROW, &time_spans(app));

    if let Some(why) = &app.drawn {
        centred(out, x, HOME_W, y + 24, "LAST GAME DRAWN", ALARM_TOP);
        for line in wrap(why, HOME_W as usize - 6).iter().take(1) {
            centred(out, x, HOME_W, y + 25, line, TEXT_DIM);
        }
    }
    let help = keys(&[("↑↓", "mode"), ("←→", "time"), ("ENTER", "start"), ("Q", "quit")]);
    centred_spans(out, x, HOME_W, y + HOME_H - 4, &help);
}

/// One menu button with an emoji before its label, glowing when chosen.
#[allow(clippy::too_many_arguments)]
fn button(cv: &mut Canvas, out: &mut Vec<Text>, x: i32, by: i32, name: &str, glyph: char, chosen: bool, pulse: f32) {
    let (bw, bx) = (BUTTON_W, x + BUTTON_X);
    bevel_box(cv, bx, by, bw, 3, chosen.then(|| AMBER.scale(pulse)));
    let colour = if chosen { AMBER_TOP.scale(pulse) } else { TEXT };
    // The emoji (two cells wide, keeping its own colours) and the name are
    // centred together; the marker hangs to their left.
    let start = bx + (bw - (3 + name.chars().count() as i32)) / 2;
    if chosen {
        out.push(Text::new(start - 3, by + 1, "▶".to_string(), colour).bold());
    }
    out.push(Text::new(start, by + 1, glyph.to_string(), colour));
    out.push(Text::new(start + 3, by + 1, name.to_string(), colour).bold());
}

/// A help line of keys and what they do.
fn keys(items: &[(&str, &str)]) -> Spans {
    let mut spans = Vec::new();
    for (i, (key, what)) in items.iter().enumerate() {
        let gap = if i + 1 < items.len() { "  " } else { "" };
        spans.push((key.to_string(), AMBER, true));
        spans.push((format!(" {what}{gap}"), TEXT_FAINT, false));
    }
    spans
}

/// `s` broken into lines of at most `w` characters, at spaces where it can
/// be (a long hash is cut anywhere).
fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split(' ') {
        let chars: Vec<char> = word.chars().collect();
        // A piece of a long word fills a line by itself, so it never gets a
        // space before it.
        for piece in chars.chunks(w) {
            if !line.is_empty() && line.chars().count() + 1 + piece.len() > w {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.extend(piece);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The VS COMPUTER settings and the row each is on in the home box.
const SETUP_ROWS: [(Setting, i32); 3] = [(Setting::Level, 9), (Setting::Colour, 11), (Setting::Time, 13)];
/// The play button: its row and width.
const PLAY_ROW: i32 = 15;
const PLAY_W: i32 = 20;

/// A VS COMPUTER setting with arrows to change it; bright when it has the
/// focus.
fn setting_spans(app: &App, setting: Setting) -> Spans {
    let (name, value) = match setting {
        Setting::Level => ("LEVEL  ", engine::LEVELS[app.setup.level.min(engine::LEVELS.len() - 1)].name.to_string()),
        Setting::Colour => ("PLAY AS  ", app.setup.colour.name().to_string()),
        _ => ("TIME  ", time_control(app)),
    };
    let focus = app.setup.focus == setting;
    let arrows = if focus { AMBER } else { TEXT_FAINT };
    vec![
        (name.into(), TEXT_DIM, false),
        ("◀ ".into(), arrows, true),
        (value, if focus { AMBER_TOP } else { TEXT }, true),
        (" ▶".into(), arrows, true),
    ]
}

/// The VS COMPUTER setup, inside the home box from row 8 down: level,
/// colour and time, the play button, and how Stockfish is getting on (its
/// download the first time).
fn setup(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, t: f32) {
    let pulse = 0.75 + 0.25 * (t * 3.0).sin();
    for (setting, row) in SETUP_ROWS {
        centred_spans(out, x, HOME_W, y + row, &setting_spans(app, setting));
    }

    let ready = matches!(app.engine, EngineStatus::Ready { .. });
    let focus = app.setup.focus == Setting::Play;
    let bx = x + (HOME_W - PLAY_W) / 2;
    bevel_box(cv, bx, y + PLAY_ROW, PLAY_W, 3, (focus && ready).then(|| AMBER.scale(pulse)));
    let (label, colour) = match (ready, focus) {
        (true, true) => ("▶ PLAY", AMBER_TOP.scale(pulse)),
        (true, false) => ("PLAY", TEXT),
        (false, _) => ("PLAY", TEXT_FAINT),
    };
    centred(out, bx, PLAY_W, y + PLAY_ROW + 1, label, colour);

    let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(t * 10.0) as usize % 10];
    let help = match &app.engine {
        EngineStatus::Unknown | EngineStatus::Starting => {
            centred(out, x, HOME_W, y + 20, &format!("{spinner} starting Stockfish…"), AMBER);
            keys(&[("↑↓", "choose"), ("←→", "change"), ("ESC", "back")])
        }
        EngineStatus::Downloading { got, total } => {
            centred(out, x, HOME_W, y + 19, &format!("{spinner} DOWNLOADING {}", engine::NAME.to_uppercase()), AMBER);
            // A sunken bar, filled in amber as the download comes in.
            let (bw, bx) = (34, x + (HOME_W - 34) / 2);
            let k = cv.scale();
            let (px, py, pw, ph) = (bx * k, 2 * (y + 21) * k, bw * k, 2 * k);
            cv.fill_rect(px - k, py - k, pw + 2 * k, ph + 2 * k, PANEL_OUTLINE);
            cv.fill_rect(px, py, pw, ph, SHADOW);
            let done = (pw as u64 * got / (*total).max(1)).min(pw as u64) as i32;
            cv.fill_rect(px, py, done, ph / 2, AMBER_TOP);
            cv.fill_rect(px, py + ph / 2, done, ph - ph / 2, AMBER_BOTTOM);
            let mb = |b: u64| b / 1_000_000;
            centred(out, x, HOME_W, y + 23, &format!("{} of {} MB · only the first time", mb(*got), mb(*total)), TEXT_DIM);
            keys(&[("↑↓", "choose"), ("←→", "change"), ("ESC", "back")])
        }
        EngineStatus::Ready { name } => {
            centred(out, x, HOME_W, y + 20, &format!("{name} is ready"), TEXT_DIM);
            keys(&[("↑↓", "choose"), ("←→", "change"), ("ENTER", "play"), ("ESC", "back")])
        }
        EngineStatus::Failed(why) => {
            centred(out, x, HOME_W, y + 19, "COULD NOT GET STOCKFISH", ALARM_TOP);
            for (i, line) in wrap(why, HOME_W as usize - 6).iter().take(5).enumerate() {
                centred(out, x, HOME_W, y + 20 + i as i32, line, TEXT_DIM);
            }
            keys(&[("ENTER", "try again"), ("ESC", "back")])
        }
    };
    centred_spans(out, x, HOME_W, y + HOME_H - 4, &help);
}

/// The peer-to-peer lobby, inside the home box from row 8 down.
fn lobby(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, now: Instant, t: f32) {
    let pulse = 0.75 + 0.25 * (t * 3.0).sin();
    let help = match &app.lobby {
        Lobby::Choose { pick } => {
            for (i, p) in Pick::ALL.into_iter().enumerate() {
                button(cv, out, x, y + BUTTONS_ROW + 4 * i as i32, p.name(), pick_glyph(p), p == *pick, pulse);
            }
            if *pick == Pick::Join {
                centred(out, x, HOME_W, y + HOME_TIME_ROW, "the host chooses the time", TEXT_FAINT);
            } else {
                centred_spans(out, x, HOME_W, y + HOME_TIME_ROW, &time_spans(app));
            }
            let hint = match pick {
                Pick::Host => "anywhere, through the game server",
                Pick::HostLan => "same Wi-Fi or hotspot, no server needed",
                Pick::Join => "with the code on the host's screen",
            };
            centred(out, x, HOME_W, y + 24, hint, TEXT_DIM);
            keys(&[("↑↓", "choose"), ("←→", "time"), ("ENTER", "go"), ("ESC", "back")])
        }
        Lobby::Code { input, error } => {
            centred(out, x, HOME_W, y + 8, "ENTER THE GAME CODE", AMBER);
            let (bw, bx) = (34, x + (HOME_W - 34) / 2);
            bevel_box(cv, bx, y + 10, bw, 3, Some(AMBER.scale(pulse)));
            let caret = if (t * 2.0).fract() < 0.5 { "▌" } else { " " };
            out.push(Text::new(bx + 3, y + 11, format!("{input}{caret}"), AMBER_TOP).bold());
            centred(out, x, HOME_W, y + 15, "it is on the host's screen", TEXT_DIM);
            if *error {
                centred(out, x, HOME_W, y + 17, "not a game code (like 7-comet-pilot)", ALARM_TOP);
            }
            keys(&[("ENTER", "join"), ("ESC", "back")])
        }
        Lobby::Connecting { stage, code, .. } => {
            if let Some(code) = code {
                centred(out, x, HOME_W, y + 8, "YOUR GAME CODE", TEXT_DIM);
                let (bx, bw) = (x + CODE_BOX.0, CODE_BOX.1);
                bevel_box(cv, bx, y + CODE_ROW, bw, 3, Some(AMBER.scale(pulse)));
                centred(out, bx, bw, y + CODE_ROW + 1, code, AMBER_TOP);
                // The copy button lights up for a moment after each copy.
                let copied = app.copied_showing(now);
                let (cx, cw) = (x + COPY_BUTTON.0, COPY_BUTTON.1);
                let (label, colour, glow) = match copied {
                    Some(true) => ("COPIED", AMBER_TOP, Some(AMBER)),
                    Some(false) => ("FAILED", ALARM_TOP, Some(ALARM_BOTTOM)),
                    None => ("COPY", TEXT, None),
                };
                bevel_box(cv, cx, y + CODE_ROW, cw, 3, glow);
                centred(out, cx, cw, y + CODE_ROW + 1, label, colour);
                let note = match app.copied {
                    Some((_, true)) => "copied: paste it to the other player",
                    Some((_, false)) => "no clipboard: tell it to the other player",
                    None => "tell it to the other player",
                };
                centred(out, x, HOME_W, y + 14, note, TEXT_DIM);
            }
            let what = match stage {
                Stage::Server => "contacting the server",
                Stage::Searching if app.lobby.pick() == Pick::HostLan => "getting ready",
                Stage::Searching => "looking on this network",
                Stage::Code(_) => "waiting for the other player",
                Stage::Punching => "connecting directly",
                Stage::Relay => "connecting through the relay",
                Stage::Handshake => "checking the code",
            };
            let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(t * 10.0) as usize % 10];
            centred(out, x, HOME_W, y + 17, &format!("{spinner} {what}…"), AMBER);
            let place = if app.lobby.lan() { "on this network, no server".to_string() } else { format!("server {}", app.server) };
            centred(out, x, HOME_W, y + 19, &place, TEXT_FAINT);
            if code.is_some() { keys(&[("C", "copy"), ("ESC", "cancel")]) } else { keys(&[("ESC", "cancel")]) }
        }
        Lobby::Failed { failure, .. } => {
            centred(out, x, HOME_W, y + 8, failure.title, ALARM_TOP);
            let lines = wrap(&failure.message, HOME_W as usize - 6);
            for (i, line) in lines.iter().take(6).enumerate() {
                centred(out, x, HOME_W, y + 10 + i as i32, line, TEXT);
            }
            if let Some(detail) = &failure.detail {
                let top = y + 11 + lines.len().min(6) as i32;
                for (i, line) in wrap(detail, HOME_W as usize - 6).iter().take(3).enumerate() {
                    centred(out, x, HOME_W, top + i as i32, line, TEXT_FAINT);
                }
            }
            keys(&[("ENTER", "try again"), ("ESC", "back")])
        }
    };
    centred_spans(out, x, HOME_W, y + HOME_H - 4, &help);
}

/// The box for a move being typed: what was typed, then up to five legal
/// moves it could be, the one Enter plays highlighted.
fn move_input(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, w: i32, h: i32) {
    let Some(input) = &app.input else { return };
    bevel_box(cv, x, y, w, h, Some(AMBER));
    out.push(Text::new(x + 2, y + 1, "TYPE A MOVE", AMBER).bold());
    let typed = format!("› {}▌", input.text);
    out.push(Text::new(x + w - 3 - typed.chars().count() as i32, y + 1, typed, AMBER_TOP).bold());
    let found = app.suggestions();
    if found.is_empty() {
        out.push(Text::new(x + 3, y + 2, "no legal move matches", TEXT_FAINT));
        return;
    }
    let pos = app.game.position();
    for (i, s) in found.iter().take(app::SUGGESTIONS).enumerate() {
        let chosen = i == input.choice;
        let colour = if chosen { AMBER_TOP } else { TEXT };
        let piece = pos.piece_at(s.mv.from).map_or(String::new(), |p| format!("{:?}", p.kind).to_lowercase());
        let from_to = format!("{} {} → {}", piece, chess_core::name(s.mv.from), chess_core::name(s.mv.to));
        let mut row = Text::new(x + 2, y + 2 + i as i32, format!(" {:<8}{:<w$}", s.san, from_to, w = (w - 14) as usize), colour);
        if chosen {
            row = row.on(ROW_HIGHLIGHT).bold();
        }
        out.push(row);
    }
    let more = found.len().saturating_sub(app::SUGGESTIONS);
    if more > 0 {
        out.push(Text::new(x + 3, y + 2 + app::SUGGESTIONS as i32, format!("+{more} more: keep typing"), TEXT_FAINT));
    }
}

/// The box about a network game: colour, connection, and anything gone wrong.
fn online(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, w: i32, h: i32) {
    let Some(o) = &app.online else { return };
    let alarm = o.dropped.is_some();
    bevel_box(cv, x, y, w, h, alarm.then_some(ALARM_BOTTOM));
    out.push(Text::new(x + 2, y + 1, "ONLINE", AMBER).bold());
    let via = match o.path.via {
        Via::Relay => "relay",
        Via::Lan => "lan",
        _ => "direct",
    };
    let link = format!("{via}  {} ms", o.rtt.as_millis());
    out.push(Text::new(x + w - 3 - link.chars().count() as i32, y + 1, link, TEXT_DIM));
    out.push(Text::new(x + 2, y + 2, format!("you play {}", o.me().name()), TEXT));

    let (message, colour) = if let Some(why) = &o.dropped {
        (format!("GAME DROPPED: {why}"), ALARM_TOP)
    } else if o.leaving.is_some() {
        ("press R again to leave; the game ends with no result".to_string(), AMBER)
    } else if o.silent {
        ("the other player went silent past their time".to_string(), TEXT_DIM)
    } else if o.peer_left {
        ("the other player has left".to_string(), TEXT_DIM)
    } else {
        (String::new(), TEXT)
    };
    for (i, line) in wrap(&message, (w - 4) as usize).iter().take((h - 4) as usize).enumerate() {
        out.push(Text::new(x + 2, y + 3 + i as i32, line.clone(), colour));
    }
}

/// The box about a game against the computer: the level, the colours, and
/// whether it is thinking or has failed.
#[allow(clippy::too_many_arguments)]
fn versus(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, w: i32, h: i32, now: Instant) {
    let Some(v) = &app.versus else { return };
    bevel_box(cv, x, y, w, h, v.error.is_some().then_some(ALARM_BOTTOM));
    out.push(Text::new(x + 2, y + 1, "VS COMPUTER", AMBER).bold());
    let level = engine::LEVELS[v.level.min(engine::LEVELS.len() - 1)];
    let tag = format!("level {} · {}", v.level + 1, level.name);
    out.push(Text::new(x + w - 3 - tag.chars().count() as i32, y + 1, tag, TEXT_DIM));
    let name = match &app.engine {
        EngineStatus::Ready { name } => name.as_str(),
        _ => "Stockfish",
    };
    out.push(Text::new(x + 2, y + 2, format!("you play {} against {name}", v.human.name()), TEXT));
    if let Some(why) = &v.error {
        let message = format!("GAME STOPPED: {why}");
        for (i, line) in wrap(&message, (w - 4) as usize).iter().take((h - 3) as usize).enumerate() {
            out.push(Text::new(x + 2, y + 3 + i as i32, line.clone(), ALARM_TOP));
        }
    } else if v.asked.is_some() && !app.over() {
        let t = now.saturating_duration_since(app.started).as_secs_f32();
        let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][(t * 10.0) as usize % 10];
        out.push(Text::new(x + 2, y + 3, format!("{spinner} thinking…"), AMBER));
    }
}

/// Big pixel digits for the countdown, 5x7, one string per row.
fn glyph(n: u64) -> [&'static str; 7] {
    match n {
        1 => ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
        2 => [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
        _ => [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    }
}

/// The pop-up over the dimmed board while the game counts down to its
/// start: the mode, the time control and a big 3, 2, 1. Drawn after the
/// pieces, on whatever scale the canvas has.
pub fn countdown_popup(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, g: &BoardGeom, now: Instant) {
    let Some(n) = app.countdown(now) else { return };
    let k = cv.scale();
    cv.blend_rect((g.x + g.f) * k, (g.y + g.f) * k, 8 * g.s * k, 8 * g.s * k, SHADOW, 0.5);
    let (w, h) = (24, 14);
    let x = g.x + (g.outer() - w) / 2;
    let y = (g.y + g.outer() / 2) / 2 - h / 2;
    bevel_box(cv, x, y, w, h, None);
    let title = match app.me() {
        Some(me) => format!("YOU PLAY {}", me.name()),
        None => app.mode.name().to_string(),
    };
    centred(out, x, w, y + 1, &title, AMBER);
    centred(out, x, w, y + 2, &time_control(app), TEXT_DIM);

    // Each digit lands bright and cools over its second. Font pixels are
    // 2x2 canvas pixels, lit from the top with a drop shadow.
    let frac = match app.screen {
        Screen::Countdown { go } => go.saturating_duration_since(now).as_secs_f32().fract(),
        _ => 0.0,
    };
    let glow = 0.8 + 0.2 * frac;
    let (px, py, u) = ((x + (w - 10) / 2) * k, 2 * (y + 4) * k, 2 * k);
    for (j, row) in glyph(n).iter().enumerate() {
        for (i, _) in row.chars().enumerate().filter(|&(_, c)| c == '#') {
            let (cx, cy) = (px + i as i32 * u, py + j as i32 * u);
            cv.blend_rect(cx + k, cy + k, u, u, SHADOW, 0.8);
        }
    }
    for (j, row) in glyph(n).iter().enumerate() {
        let c = AMBER_TOP.lerp(AMBER_BOTTOM, j as f32 / 6.0).scale(glow);
        for (i, _) in row.chars().enumerate().filter(|&(_, c)| c == '#') {
            cv.fill_rect(px + i as i32 * u, py + j as i32 * u, u, u, c);
        }
    }
    centred_spans(out, x, w, y + h - 2, &keys(&[("ESC", "back to menu")]));
}

/// The game-over pop-up, `END_W` x `END_H` cells centred on the board,
/// and its two buttons: their row from the top, width, and columns from
/// the left edge.
const END_W: i32 = 34;
const END_H: i32 = 19;
const END_BUTTONS_ROW: i32 = 14;
const END_BUTTON_W: i32 = 14;
const END_BUTTONS: [(EndChoice, &str, i32); 2] = [(EndChoice::NewGame, "NEW GAME", 2), (EndChoice::Home, "HOME", 18)];

/// The pop-up's top-left cell over the board `g`.
fn end_origin(g: &BoardGeom) -> (i32, i32) {
    (g.x + (g.outer() - END_W) / 2, (g.y + g.outer() / 2) / 2 - END_H / 2)
}

/// Which of the pop-up's buttons the cell (`col`, `row`) is on.
pub fn end_hit(g: &BoardGeom, col: i32, row: i32) -> Option<EndChoice> {
    let (x, y) = end_origin(g);
    let row_ok = (y + END_BUTTONS_ROW..y + END_BUTTONS_ROW + 3).contains(&row);
    END_BUTTONS
        .into_iter()
        .find(|&(_, _, dx)| row_ok && (x + dx..x + dx + END_BUTTON_W).contains(&col))
        .map(|(choice, _, _)| choice)
}

/// The pop-up's pictures, drawn like the countdown's digits: `#` is the
/// main colour, `+` light, `-` dark, a space clear.
const TROPHY: [&str; 7] = [
    "#########",
    "# +###- #",
    " #+###-# ",
    "  #####  ",
    "    #    ",
    "   ###   ",
    " ####### ",
];
const KING: [&str; 7] = [
    "    #    ",
    "   ###   ",
    "    #    ",
    "  #####  ",
    "   #-#   ",
    "   #-#   ",
    " ####### ",
];
const SCALES: [&str; 7] = [
    "    #    ",
    "#########",
    "#   #   #",
    "#   #   #",
    "### # ###",
    "    #    ",
    "  #####  ",
];

/// The end of a game, over the dimmed board: what happened, a picture (a
/// trophy for a win, with confetti falling over the board; a king for a
/// loss online; scales for a draw), and NEW GAME and HOME. Drawn after the
/// pieces, on whatever scale the canvas has.
pub fn end_popup(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, g: &BoardGeom, now: Instant) {
    let Some((winner, reason)) = app.result() else { return };
    let since = app.ended_at.map_or(0.0, |at| now.saturating_duration_since(at + app::END_PAUSE).as_secs_f32());
    let k = cv.scale();
    let (bx, by, side) = ((g.x + g.f) * k, (g.y + g.f) * k, 8 * g.s * k);
    cv.blend_rect(bx, by, side, side, SHADOW, 0.55);

    let me = app.online.as_ref().map(|o| o.me());
    let (headline, art, won) = match (winner, me) {
        (None, _) => ("DRAW".to_string(), &SCALES, false),
        (Some(w), Some(me)) if w == me => ("YOU WIN".to_string(), &TROPHY, true),
        (Some(_), Some(_)) => ("YOU LOST".to_string(), &KING, false),
        (Some(w), None) => (format!("{} WINS", w.name()), &TROPHY, true),
    };
    if won {
        confetti(cv, bx, by, side, since);
    }

    let (x, y) = end_origin(g);
    let pulse = 0.85 + 0.15 * (since * 3.0).sin();
    bevel_box(cv, x, y, END_W, END_H, won.then(|| AMBER.scale(pulse)));
    centred(out, x, END_W, y + 1, &reason.to_uppercase(), TEXT_DIM);

    // The picture lands big, two canvas pixels a dot, with a drop shadow.
    let lost = winner.is_some() && !won;
    let (main, light, dark) = if lost {
        (IDLE_TOP, TEXT, IDLE_BOTTOM)
    } else {
        (AMBER.scale(pulse), AMBER_TOP.scale(pulse), AMBER_BOTTOM.scale(0.7 * pulse))
    };
    let (px, py, u) = ((x + (END_W - 2 * art[0].len() as i32) / 2) * k, 2 * (y + 3) * k, 2 * k);
    let dots = || art.iter().enumerate().flat_map(|(j, row)| row.chars().enumerate().map(move |(i, c)| (i as i32, j as i32, c)));
    for (i, j, c) in dots() {
        if c != ' ' {
            cv.blend_rect(px + i * u + k, py + j * u + k, u, u, SHADOW, 0.8);
        }
    }
    for (i, j, c) in dots() {
        let colour = match c {
            '#' => main,
            '+' => light,
            '-' => dark,
            _ => continue,
        };
        cv.fill_rect(px + i * u, py + j * u, u, u, colour);
    }

    centred(out, x, END_W, y + 11, &headline, if lost { TEXT } else { AMBER_TOP });
    let moves = app.history.len().div_ceil(2);
    let after = format!("after {moves} move{}", if moves == 1 { "" } else { "s" });
    centred(out, x, END_W, y + 12, &after, TEXT_DIM);

    for (choice, label, dx) in END_BUTTONS {
        let chosen = app.end_choice == choice;
        let (cx, cy) = (x + dx, y + END_BUTTONS_ROW);
        bevel_box(cv, cx, cy, END_BUTTON_W, 3, chosen.then(|| AMBER.scale(pulse)));
        let (text, colour) = if chosen { (format!("▶ {label}"), AMBER_TOP) } else { (label.to_string(), TEXT) };
        centred(out, cx, END_BUTTON_W, cy + 1, &text, colour);
    }
    centred_spans(out, x, END_W, y + END_H - 2, &keys(&[("ENTER", "go"), ("ESC", "see board")]));
}

/// Confetti falling over the board square at (`x`, `y`), `side` pixels
/// wide, `t` seconds after the pop-up came: each piece has its own column,
/// speed, sway and colour, and turns as it falls. It thins out after a few
/// seconds and stops, so the pop-up settles.
fn confetti(cv: &mut Canvas, x: i32, y: i32, side: i32, t: f32) {
    const COLOURS: [Rgb; 5] = [AMBER_TOP, CURSOR, ALARM_TOP, PLY_WHITE, LAST_MOVE];
    let k = cv.scale();
    for i in 0..90 {
        let r = |n: i32| hash(i, n, 0xc0ff);
        // Each piece starts above the board at its own moment and falls once.
        let start = r(1) * 2.5;
        let fall = t - start;
        if fall < 0.0 {
            continue;
        }
        let speed = side as f32 * (0.35 + 0.35 * r(2));
        let py = y as f32 - 4.0 * k as f32 + fall * speed;
        if py >= (y + side) as f32 {
            continue;
        }
        let sway = (fall * (2.0 + 2.0 * r(3)) + r(4) * 6.3).sin() * 3.0 * k as f32;
        let px = x as f32 + r(5) * side as f32 + sway;
        // Turning: wide, then edge-on, then wide again.
        let wide = (fall * (5.0 + 4.0 * r(6))).cos().abs() > 0.4;
        let (w, h) = if wide { (2 * k, k) } else { (k, k) };
        let colour = COLOURS[(r(7) * COLOURS.len() as f32) as usize % COLOURS.len()];
        let (px, py) = (px as i32, py as i32);
        if px >= x && px + w <= x + side && py >= y {
            cv.fill_rect(px, py, w, h, colour);
        }
    }
}

/// Where captured pieces start in a strip, and the columns each takes.
pub fn captured_x(r: Cells) -> i32 {
    r.x + 20
}
pub const CAPTURED_COLS: i32 = 4;

/// A slim bar beside the board for one player: name and turn on the left,
/// the pieces they have captured in the middle, a small clock on the right.
/// Text goes on `text_row`, the row nearer the board. With `icons`, the
/// renderer draws the captured pieces as pixel art; otherwise they are
/// letters here.
#[allow(clippy::too_many_arguments)]
pub fn strip(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, side: Side, r: Cells, text_row: i32, icons: bool, now: Instant, t: f32) {
    let clock = &app.clock;
    let flagged = clock.flagged == Some(side);
    let live = clock.running() == Some(side);
    let result = app.result();
    let turn = app.to_move() == side && !app.over();
    let check = turn && app.game.position().in_check();
    let left = clock.remaining(side, now);
    let low = left.as_secs() < 20 && live;

    cv.blend_rect(r.x, 2 * r.y, r.w, 2 * r.h, PANEL, 0.92);
    if turn {
        cv.fill_rect(r.x, 2 * r.y, 1, 2 * r.h, AMBER);
    }

    out.push(Text::new(r.x + 2, text_row, side.name(), if turn || flagged { TEXT } else { TEXT_DIM }).bold());
    let blink = (t * 3.0).sin() > -0.3;
    let status = if let Some((winner, reason)) = result {
        match winner {
            Some(w) if w == side => Some(("WINS", AMBER)),
            _ if flagged => Some(("TIME!", ALARM_TOP)),
            Some(_) if reason == "checkmate" => Some(("CHECKMATE", ALARM_TOP)),
            Some(_) if reason == "resignation" => Some(("RESIGNED", ALARM_TOP)),
            Some(_) => None,
            None => Some(("DRAW", TEXT)),
        }
    } else if check {
        Some((if blink { "● CHECK" } else { "  CHECK" }, ALARM_TOP))
    } else if app.draw_offer == Some(side) {
        Some(("DRAW OFFER", AMBER))
    } else if live && app.versus.as_ref().is_some_and(|v| v.human != side) {
        Some((if blink { "● THINKING" } else { "  THINKING" }, AMBER))
    } else if live {
        Some((if blink { "● TO MOVE" } else { "  TO MOVE" }, AMBER))
    } else if turn && clock.paused() {
        Some(("PAUSED", TEXT_DIM))
    } else if turn {
        Some(("READY", AMBER))
    } else {
        None
    };
    if let Some((s, c)) = status {
        out.push(Text::new(r.x + 8, text_row, s, c));
    }

    let taken = app.captured_by(side);
    let mut x = captured_x(r);
    if icons {
        x += taken.len() as i32 * CAPTURED_COLS;
    } else {
        for p in &taken {
            let letter = p.kind.letter().to_string();
            let c = if p.color == Side::White { PLY_WHITE } else { PLY_BLACK };
            out.push(Text::new(x, text_row, letter, c).bold());
            x += 2;
        }
    }
    let lead = app.material_lead(side);
    if lead > 0 {
        out.push(Text::new(x + 1, text_row, format!("+{lead}"), TEXT_DIM));
    }

    let digits = format!(" {} ", clock::format(left));
    let colour = if flagged || low {
        ALARM_TOP.scale(if low { 0.8 + 0.2 * (t * 8.0).sin() } else { 1.0 })
    } else if live {
        AMBER_TOP
    } else {
        IDLE_TOP
    };
    let len = digits.chars().count() as i32;
    out.push(Text::new(r.x + r.w - 1 - len, text_row, digits, colour).bold().on(LCD));
}

fn history(cv: &mut Canvas, out: &mut Vec<Text>, app: &App, x: i32, y: i32, w: i32, h: i32) {
    bevel_box(cv, x, y, w, h, None);
    let tc = app.clock.control();
    out.push(Text::new(x + 2, y + 1, "MOVES", AMBER).bold());
    let control = format!("{} {}", tc.name(), tc.label);
    out.push(Text::new(x + w - 3 - control.len() as i32, y + 1, control, TEXT_DIM));

    // The last row shows the result once the game is over.
    let result = app.result().map(|(winner, reason)| {
        let score = match winner {
            Some(Side::White) => "1-0",
            Some(Side::Black) => "0-1",
            None => "½-½",
        };
        format!("{score}  {reason}")
    });
    let rows = (h - 3 - result.is_some() as i32).max(0) as usize;
    if app.history.is_empty() {
        let msg = "no moves yet";
        out.push(Text::new(x + (w - msg.len() as i32) / 2, y + 2 + (rows as i32 - 1) / 2, msg, TEXT_FAINT));
        return;
    }
    let pairs: Vec<&[MoveRecord]> = app.history.chunks(2).collect();
    let first = pairs.len().saturating_sub(rows);
    let last_ply = app.history.len() - 1;
    const COL_W: i32 = 10;
    for (i, pair) in pairs[first..].iter().enumerate() {
        let n = first + i;
        let row = y + 2 + i as i32;
        out.push(Text::new(x + 2, row, format!("{:>3}.", n + 1), TEXT_FAINT));
        for (j, m) in pair.iter().enumerate() {
            let color = if m.piece.color == Side::White { PLY_WHITE } else { PLY_BLACK };
            let cell = format!(" {:<w$}", m.san, w = COL_W as usize - 1);
            let mut item = Text::new(x + 6 + j as i32 * COL_W, row, cell, color);
            if n * 2 + j == last_ply {
                item = item.on(ROW_HIGHLIGHT).bold();
            }
            out.push(item);
        }
    }
    if let Some(line) = result {
        let row = y + 2 + (pairs.len() - first) as i32;
        out.push(Text::new(x + 3, row, line, AMBER).bold());
    }
}

type Spans = Vec<(String, Rgb, bool)>;

fn help_lines(app: &App, w: i32) -> Vec<Spans> {
    let online = app.online.is_some();
    let versus = app.versus.is_some();
    if app.input.is_some() {
        return vec![keys(&[("↑↓", "choose"), ("ENTER", "play"), ("ESC", "cancel")])];
    }
    let keys = [
        ("←↑↓→", "move"),
        ("SPACE", "pick/drop"),
        ("CLICK", "pick/drop"),
        ("Nf3", "type a move"),
        ("D", "draw"),
        ("X", "resign"),
        ("ESC", "cancel"),
        ("F", "flip"),
        ("A", if app.auto_flip { "auto flip: on" } else { "auto flip: off" }),
        ("G", "hi-res"),
        ("P", "pause"),
        ("R", if online { "leave" } else { "menu" }),
        ("Q", "quit"),
    ];
    let mut lines = Vec::new();
    let mut line: Spans = Vec::new();
    let mut len = 0;
    for (key, desc) in keys.into_iter().filter(|&(k, _)| !(online && (k == "P" || k == "A")) && !(versus && (k == "A" || k == "D"))) {
        let item = key.chars().count() as i32 + 1 + desc.len() as i32;
        if len > 0 && len + 2 + item > w {
            lines.push(std::mem::take(&mut line));
            len = 0;
        }
        if len > 0 {
            line.push(("  ".into(), TEXT_DIM, false));
            len += 2;
        }
        line.push((key.into(), AMBER, true));
        line.push((format!(" {desc}"), TEXT_FAINT, false));
        len += item;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn put_spans(out: &mut Vec<Text>, x: i32, y: i32, spans: &Spans) {
    let mut cx = x;
    for (s, c, bold) in spans {
        let mut t = Text::new(cx, y, s.clone(), *c);
        if *bold {
            t = t.bold();
        }
        cx += s.chars().count() as i32;
        out.push(t);
    }
}

/// The message shown instead of the board when the window is the wrong
/// size for the layout: plain text, so it fits any window, `w` x `h` cells.
pub fn zoom_message(zoom_in: bool, w: i32, h: i32) -> Vec<Text> {
    let key = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
    let (c, r) = layout::needed();
    let mut lines = if zoom_in {
        vec![
            ("ZOOM IN FOR A BIGGER BOARD".to_string(), AMBER),
            (format!("press {key} + until the board appears"), TEXT),
            ("or press ENTER to play at this size".to_string(), TEXT_DIM),
        ]
    } else {
        vec![("ZOOM OUT TO PLAY".to_string(), AMBER), (format!("press {key} − until the board appears"), TEXT)]
    };
    lines.push((format!("window {w}×{h} · board needs {c}×{r}"), TEXT_FAINT));
    let y0 = (h - lines.len() as i32) / 2;
    lines
        .into_iter()
        .enumerate()
        .map(|(i, (s, c))| Text::new((w - s.chars().count() as i32) / 2, y0 + i as i32, s, c).bold())
        .collect()
}
