/// Mouse cursor rendering for Smart OS.
///
/// A 12x16 pixel neon arrow cursor rendered on top of the desktop.

use super::compositor::Compositor;
use super::theme::{ACCENT_CYAN, Color};

/// Cursor width in pixels.
pub const CURSOR_W: usize = 12;
/// Cursor height in pixels.
pub const CURSOR_H: usize = 16;

/// Cursor bitmap: 1 = white fill, 2 = outline (dark), 0 = transparent.
/// Classic arrow pointer shape.
#[rustfmt::skip]
static CURSOR_DATA: [[u8; CURSOR_W]; CURSOR_H] = [
    [1,0,0,0,0,0,0,0,0,0,0,0],
    [1,1,0,0,0,0,0,0,0,0,0,0],
    [1,2,1,0,0,0,0,0,0,0,0,0],
    [1,2,2,1,0,0,0,0,0,0,0,0],
    [1,2,2,2,1,0,0,0,0,0,0,0],
    [1,2,2,2,2,1,0,0,0,0,0,0],
    [1,2,2,2,2,2,1,0,0,0,0,0],
    [1,2,2,2,2,2,2,1,0,0,0,0],
    [1,2,2,2,2,2,2,2,1,0,0,0],
    [1,2,2,2,2,2,2,2,2,1,0,0],
    [1,2,2,2,2,2,1,1,1,1,1,0],
    [1,2,2,1,2,2,1,0,0,0,0,0],
    [1,2,1,0,1,2,2,1,0,0,0,0],
    [1,1,0,0,1,2,2,1,0,0,0,0],
    [1,0,0,0,0,1,2,2,1,0,0,0],
    [0,0,0,0,0,1,1,1,0,0,0,0],
];

/// Draw the mouse cursor at (x, y) with a neon glow effect.
pub fn draw_cursor(comp: &mut Compositor, x: usize, y: usize) {
    let outline_color = Color::rgb(0, 0, 0);       // Black outline
    let fill_color = Color::rgb(255, 255, 255);     // White fill
    let glow_color = ACCENT_CYAN;                    // Neon glow

    // Draw subtle glow (1px around the cursor shape)
    for row in 0..CURSOR_H {
        for col in 0..CURSOR_W {
            if CURSOR_DATA[row][col] > 0 {
                // Draw glow pixels around the cursor
                let px = x + col;
                let py = y + row;
                if px > 0 { comp.set_pixel(px - 1, py, glow_color.dim(40)); }
                if py > 0 { comp.set_pixel(px, py - 1, glow_color.dim(40)); }
                comp.set_pixel(px + 1, py, glow_color.dim(40));
                comp.set_pixel(px, py + 1, glow_color.dim(40));
            }
        }
    }

    // Draw cursor shape on top
    for row in 0..CURSOR_H {
        for col in 0..CURSOR_W {
            let pixel = CURSOR_DATA[row][col];
            if pixel > 0 {
                let color = if pixel == 1 { outline_color } else { fill_color };
                comp.set_pixel(x + col, y + row, color);
            }
        }
    }
}
