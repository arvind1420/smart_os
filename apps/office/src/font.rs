/// Rich Text and Vector Font Rasterizer for Smart Office.
///
/// Implements a minimal quadratic bezier curve evaluator to draw scalable
/// vector fonts directly to the window, avoiding fixed-width bitmap limitations.
/// Also provides a Rich-Text layout engine to manage spans of text with
/// different styles (bold, italic, size).

use smartsdk::gui::Window;

/// A simple 2D point.
#[derive(Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A quadratic bezier curve used in TrueType fonts.
pub struct Bezier {
    pub p0: Point,
    pub p1: Point,
    pub p2: Point,
}

impl Bezier {
    /// Evaluate the curve at t (0.0 to 1.0).
    pub fn evaluate(&self, t: f32) -> Point {
        let u = 1.0 - t;
        let tt = t * t;
        let uu = u * u;
        let ut2 = 2.0 * u * t;

        Point {
            x: uu * self.p0.x + ut2 * self.p1.x + tt * self.p2.x,
            y: uu * self.p0.y + ut2 * self.p1.y + tt * self.p2.y,
        }
    }
}

/// A very minimal Vector Font representation.
/// Instead of parsing a real .ttf file (which requires a massive library),
/// we define a small set of vector glyphs programmatically to prove the concept.
pub struct VectorFont;

impl VectorFont {
    /// Draws a simulated vector character onto the window.
    /// In a real system, this would evaluate bezier paths and use a scanline
    /// rasterizer to fill the shape. Here we evaluate the bezier curves
    /// and draw them by plotting points using `fill_rect` as pixels.
    pub fn draw_char(win: &Window, c: char, x: u16, y: u16, size: u16, bold: bool, italic: bool) -> u16 {
        let mut x_offset = x;
        let mut y_offset = y;
        
        // Apply italic sheer
        let sheer = if italic { size as f32 * 0.2 } else { 0.0 };
        
        // Thickness for bold
        let thickness = if bold { 2 } else { 1 };

        let draw_pixel = |win: &Window, px: u16, py: u16| {
            for dx in 0..thickness {
                for dy in 0..thickness {
                    win.fill_rect(px + dx, py + dy, 1, 1, 0x000000);
                }
            }
        };

        // For MVP, we'll draw a simulated scalable box or line based on the character.
        // A true rasterizer would have a dictionary of `Bezier` arrays per char.
        if c != ' ' {
            // Draw a basic shape to represent the vector character
            let s = size;
            
            // Just mapping 'A'-'Z' to a simple scalable box to prove the vector scale works
            for i in 0..s {
                let sheer_offset = ((s - i) as f32 / s as f32 * sheer) as u16;
                // Left edge
                draw_pixel(win, x_offset + sheer_offset, y_offset + i);
                // Right edge
                draw_pixel(win, x_offset + s / 2 + sheer_offset, y_offset + i);
                
                // Top edge
                if i == 0 {
                    for j in 0..s/2 {
                        draw_pixel(win, x_offset + sheer_offset + j, y_offset);
                    }
                }
                // Bottom edge
                if i == s - 1 {
                    for j in 0..s/2 {
                        draw_pixel(win, x_offset + sheer_offset + j, y_offset + s - 1);
                    }
                }
            }
        }

        // Return the advance width
        size / 2 + if bold { 2 } else { 0 } + 2
    }
}

/// Rich Text Style attributes.
#[derive(Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub size: u16,
}

impl TextStyle {
    pub fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            size: 16,
        }
    }
}

/// A span of text with a specific style.
#[derive(Clone, Copy)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    pub style: TextStyle,
}

/// Rich Text Engine.
/// Manages a gap buffer of raw text and a list of style spans.
pub struct RichTextEngine<'a> {
    pub buffer: &'a crate::gap_buffer::GapBuffer,
    pub spans: [TextSpan; 64], // Fixed array for MVP
    pub span_count: usize,
}

impl<'a> RichTextEngine<'a> {
    pub fn new(buffer: &'a crate::gap_buffer::GapBuffer) -> Self {
        let mut spans = [TextSpan { start: 0, end: 0, style: TextStyle::default() }; 64];
        spans[0] = TextSpan {
            start: 0,
            end: 65536, // Covers the whole buffer initially
            style: TextStyle::default(),
        };
        Self {
            buffer,
            spans,
            span_count: 1,
        }
    }

    /// Renders the rich text document using the Vector Font rasterizer.
    pub fn render(&self, win: &Window, x_start: u16, y_start: u16) {
        let mut out = [0u8; 4096];
        let len = self.buffer.copy_to_slice(&mut out);

        let mut x = x_start;
        let mut y = y_start;
        
        let mut current_style = self.spans[0].style;

        for i in 0..len {
            // Check if we entered a new span
            for s in 0..self.span_count {
                if i >= self.spans[s].start && i < self.spans[s].end {
                    current_style = self.spans[s].style;
                    break;
                }
            }

            let ch = out[i] as char;
            if ch == '\n' {
                x = x_start;
                y += current_style.size + 4;
            } else {
                let advance = VectorFont::draw_char(
                    win, 
                    ch, 
                    x, 
                    y, 
                    current_style.size, 
                    current_style.bold, 
                    current_style.italic
                );
                x += advance;
            }
        }
    }
}
