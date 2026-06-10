/// Rich Text and TrueType Font Layout Engine for Smart Office.
///
/// Integrates with the system TrueType Font API to draw scalable text,
/// computes layout metrics for exact cursor tracking, and maintains
/// style span boundaries during edits.

use smartsdk::gui::Window;

/// Rich Text Style attributes.
#[derive(Clone, Copy, PartialEq, Debug)]
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
#[derive(Clone, Copy, Debug)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    pub style: TextStyle,
}

/// Proportional character width estimation for TrueType sans-serif font
pub fn char_width(c: char, size: u16, bold: bool) -> u16 {
    let base = match c {
        'i' | 'l' | 't' | 'I' | '1' | ';' | ':' | ',' | '.' | '!' | ' ' | '\'' | '"' => 0.25,
        'f' | 'j' | 'r' | '(' | ')' | '[' | ']' | '{' | '}' | '-' | '/' => 0.35,
        'a' | 'b' | 'c' | 'd' | 'e' | 'g' | 'h' | 'k' | 'n' | 'o' | 'p' | 'q' | 's' | 'u' | 'v' | 'x' | 'y' | 'z' |
        'F' | 'J' | 'L' | 'T' | 'Z' | '0' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | '+' | '=' => 0.5,
        'A' | 'B' | 'C' | 'D' | 'E' | 'G' | 'H' | 'K' | 'N' | 'O' | 'P' | 'Q' | 'R' | 'S' | 'U' | 'V' | 'X' | 'Y' => 0.6,
        'w' | 'm' | 'W' | 'M' | '@' | '&' | '%' => 0.8,
        _ => 0.5,
    };
    let w = (size as f32 * base) as u16;
    w + if bold { 2 } else { 0 }
}

/// Rich Text Engine.
/// Manages a gap buffer of raw text and a list of style spans.
pub struct RichTextEngine<'a> {
    pub buffer: &'a crate::gap_buffer::GapBuffer,
    pub spans: &'a [TextSpan],
    pub span_count: usize,
}

impl<'a> RichTextEngine<'a> {
    pub fn new(buffer: &'a crate::gap_buffer::GapBuffer, spans: &'a [TextSpan], span_count: usize) -> Self {
        Self {
            buffer,
            spans,
            span_count,
        }
    }

    /// Renders the rich text document using the system TrueType Font API
    /// and returns the calculated cursor coordinates (x, y).
    pub fn render(&self, win: &Window, x_start: u16, y_start: u16, cursor_idx: usize) -> (u16, u16) {
        let mut out = [0u8; 8192];
        let len = self.buffer.copy_to_slice(&mut out);

        let mut x = x_start;
        let mut y = y_start;
        
        let mut current_style = TextStyle::default();
        if self.span_count > 0 {
            current_style = self.spans[0].style;
        }

        let mut cursor_coords = (x_start, y_start);

        for i in 0..len {
            // Record cursor coordinates if we hit the cursor index
            if i == cursor_idx {
                cursor_coords = (x, y);
            }

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
                // Line height proportional to current size + spacing
                y += current_style.size + 8;
            } else {
                let adv = char_width(ch, current_style.size, current_style.bold);
                
                let ch_str = [ch as u8];
                if let Ok(s) = core::str::from_utf8(&ch_str) {
                    // Draw normal text
                    win.draw_text_ttf(x, y, current_style.size, s);
                    // Draw offset text to simulate bold
                    if current_style.bold {
                        win.draw_text_ttf(x + 1, y, current_style.size, s);
                    }
                }
                x += adv;
            }
        }

        // If cursor is at the very end of text
        if cursor_idx >= len {
            cursor_coords = (x, y);
        }

        cursor_coords
    }
}

/// Helper to shift and split style spans when inserting text
pub fn adjust_spans_for_insert(
    spans: &mut [TextSpan],
    span_count: &mut usize,
    idx: usize,
    count: usize,
    active_style: TextStyle,
) {
    if *span_count == 0 {
        spans[0] = TextSpan {
            start: 0,
            end: 65536,
            style: active_style,
        };
        *span_count = 1;
        return;
    }

    // Shift all span boundaries strictly after idx first
    for i in 0..*span_count {
        if spans[i].start > idx {
            spans[i].start += count;
            spans[i].end += count;
        } else if spans[i].start <= idx && spans[i].end > idx {
            // Split or expand
            if spans[i].style == active_style {
                spans[i].end += count;
            } else {
                let old_end = spans[i].end;
                spans[i].end = idx;

                // Add the new span for the inserted text
                if *span_count < spans.len() {
                    spans[*span_count] = TextSpan {
                        start: idx,
                        end: idx + count,
                        style: active_style,
                    };
                    *span_count += 1;
                }

                // Add the remaining part of the split span
                if *span_count < spans.len() {
                    spans[*span_count] = TextSpan {
                        start: idx + count,
                        end: old_end + count,
                        style: spans[i].style,
                    };
                    *span_count += 1;
                }
                return;
            }
        } else if spans[i].end == idx {
            if spans[i].style == active_style {
                spans[i].end += count;
            }
        }
    }
}

/// Helper to shrink and remove style spans when deleting text
pub fn adjust_spans_for_delete(
    spans: &mut [TextSpan],
    span_count: &mut usize,
    idx: usize,
    count: usize,
) {
    let mut remove_indices = [false; 64];
    
    for i in 0..*span_count {
        let s = spans[i].start;
        let e = spans[i].end;

        if s >= idx + count {
            // Span is entirely after the deletion
            spans[i].start -= count;
            spans[i].end -= count;
        } else if e <= idx {
            // Span is entirely before the deletion
        } else {
            // Overlapping
            if s >= idx && e <= idx + count {
                // Entirely inside deletion range
                remove_indices[i] = true;
            } else if s < idx && e > idx + count {
                // Deletion inside span
                spans[i].end -= count;
            } else if s < idx {
                // Deletion cuts the end
                spans[i].end = idx;
            } else {
                // Deletion cuts the start
                spans[i].start = idx;
                spans[i].end -= count;
            }
        }
    }

    // Filter out deleted spans
    let mut new_spans = [TextSpan { start: 0, end: 0, style: TextStyle::default() }; 64];
    let mut new_count = 0;
    for i in 0..*span_count {
        if !remove_indices[i] && spans[i].start < spans[i].end {
            new_spans[new_count] = spans[i];
            new_count += 1;
        }
    }
    
    spans[..new_count].copy_from_slice(&new_spans[..new_count]);
    *span_count = new_count;
}
