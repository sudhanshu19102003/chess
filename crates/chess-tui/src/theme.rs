//! The palette. Warm wood for the board, a deep indigo night around it, and
//! amber for anything live.

use crate::canvas::{Rgb, rgb};

pub const BG_TOP: Rgb = rgb(0.105, 0.090, 0.170);
pub const BG_BOTTOM: Rgb = rgb(0.045, 0.040, 0.085);
pub const STAR: Rgb = rgb(0.70, 0.75, 1.0);
pub const SHADOW: Rgb = rgb(0.01, 0.0, 0.02);

pub const SQUARE_LIGHT: Rgb = rgb(0.87, 0.74, 0.55);
pub const SQUARE_DARK: Rgb = rgb(0.58, 0.39, 0.27);
pub const FRAME: Rgb = rgb(0.34, 0.205, 0.135);
pub const SLAB_TOP: Rgb = rgb(0.25, 0.145, 0.095);
pub const SLAB_BOTTOM: Rgb = rgb(0.14, 0.08, 0.055);
pub const INLAY: Rgb = rgb(0.82, 0.63, 0.32);
pub const LABEL: Rgb = rgb(0.93, 0.80, 0.55);

pub const LAST_MOVE: Rgb = rgb(0.96, 0.90, 0.42);
pub const SELECTED: Rgb = rgb(1.0, 0.76, 0.25);
pub const CURSOR: Rgb = rgb(0.38, 0.90, 1.0);
pub const MOVE_HINT: Rgb = rgb(0.16, 0.09, 0.05);
pub const CHECK: Rgb = rgb(0.95, 0.12, 0.10);


pub const PANEL: Rgb = rgb(0.115, 0.100, 0.190);
pub const PANEL_LIGHT: Rgb = rgb(0.30, 0.27, 0.46);
pub const PANEL_DARK: Rgb = rgb(0.045, 0.035, 0.085);
pub const PANEL_OUTLINE: Rgb = rgb(0.02, 0.015, 0.04);
pub const ROW_HIGHLIGHT: Rgb = rgb(0.22, 0.19, 0.36);

pub const LCD: Rgb = rgb(0.035, 0.030, 0.060);
pub const LCD_GHOST: Rgb = rgb(0.095, 0.085, 0.140);

pub const AMBER: Rgb = rgb(1.0, 0.74, 0.26);
pub const AMBER_TOP: Rgb = rgb(1.0, 0.90, 0.55);
pub const AMBER_BOTTOM: Rgb = rgb(0.98, 0.52, 0.12);
pub const IDLE_TOP: Rgb = rgb(0.60, 0.57, 0.72);
pub const IDLE_BOTTOM: Rgb = rgb(0.38, 0.35, 0.50);
pub const ALARM_TOP: Rgb = rgb(1.0, 0.55, 0.50);
pub const ALARM_BOTTOM: Rgb = rgb(0.88, 0.16, 0.18);

pub const TEXT: Rgb = rgb(0.90, 0.88, 0.96);
pub const TEXT_DIM: Rgb = rgb(0.52, 0.49, 0.64);
pub const TEXT_FAINT: Rgb = rgb(0.36, 0.33, 0.47);
pub const PLY_WHITE: Rgb = rgb(0.98, 0.94, 0.84);
pub const PLY_BLACK: Rgb = rgb(0.72, 0.68, 0.90);
