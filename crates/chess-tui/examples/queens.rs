//! Renders every queen crown style, white over black, side by side on board
//! squares, to compare them.
//!
//!     cargo run --release --example queens -- out.png [size] [zoom]

use chess_core::Color;
use chess_tui::{canvas::Canvas, pieces, theme};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "queens.png".into());
    let s: i32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(200);
    let zoom: i32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let styles = pieces::queen_styles();
    let (w, h) = (s * styles.len() as i32, 2 * s);
    let mut cv = Canvas::new(w, h);
    for (i, _) in styles.iter().enumerate() {
        for (row, side) in [Color::White, Color::Black].into_iter().enumerate() {
            let (x, y) = (i as i32 * s, row as i32 * s);
            let light = (i + row) % 2 == 0;
            cv.fill_rect(x, y, s, s, if light { theme::SQUARE_LIGHT } else { theme::SQUARE_DARK });
            let art = pieces::render_queen(i, side, s);
            cv.draw_mask(&art.shadow, x - art.pad + s / 14, y - art.pad + s / 30, theme::SHADOW, 0.32);
            cv.draw_sprite(&art.sprite, x - art.pad, y - art.pad);
        }
    }
    let (w, h) = (w * zoom, h * zoom);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = cv.get(x / zoom, y / zoom);
            rgb.extend([c.r, c.g, c.b].map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
        }
    }
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!("wrote {out}: {}", styles.join(", "));
}
