/// Gap Buffer Implementation for Smart Office.
///
/// A gap buffer allows O(1) insertions and deletions at the cursor position
/// by maintaining a "gap" of unused memory at the cursor. This solves the
/// performance issues of shifting megabytes of memory on every keystroke.

pub struct GapBuffer {
    /// The actual text buffer. For MVP, we use a fixed 64KB array to avoid
    /// needing a global allocator in user-space, but this simulates the
    /// exact same logic as a dynamically allocated gap buffer.
    buffer: [u8; 65536],
    /// Start of the gap (equals the cursor position).
    gap_start: usize,
    /// End of the gap.
    gap_end: usize,
}

impl GapBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [0; 65536],
            gap_start: 0,
            gap_end: 65536,
        }
    }

    pub fn insert(&mut self, ch: u8) {
        if self.gap_start == self.gap_end {
            // Buffer is full. In a real allocator environment, we would reallocate
            // and expand the gap. For no_std MVP without alloc, we just return.
            return;
        }
        self.buffer[self.gap_start] = ch;
        self.gap_start += 1;
    }

    pub fn delete_before_cursor(&mut self) {
        if self.gap_start > 0 {
            self.gap_start -= 1;
        }
    }

    pub fn delete_after_cursor(&mut self) {
        if self.gap_end < self.buffer.len() {
            self.gap_end += 1;
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.gap_start > 0 {
            self.gap_start -= 1;
            self.gap_end -= 1;
            self.buffer[self.gap_end] = self.buffer[self.gap_start];
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.gap_end < self.buffer.len() {
            self.buffer[self.gap_start] = self.buffer[self.gap_end];
            self.gap_start += 1;
            self.gap_end += 1;
        }
    }

    /// Read the continuous text into a target buffer (up to its capacity).
    pub fn copy_to_slice(&self, out: &mut [u8]) -> usize {
        let first_part_len = self.gap_start;
        let second_part_len = self.buffer.len() - self.gap_end;
        
        let out_first = first_part_len.min(out.len());
        out[..out_first].copy_from_slice(&self.buffer[..out_first]);
        
        let out_second = second_part_len.min(out.len() - out_first);
        if out_second > 0 {
            out[out_first..out_first + out_second]
                .copy_from_slice(&self.buffer[self.gap_end..self.gap_end + out_second]);
        }
        
        out_first + out_second
    }

    pub fn cursor_position(&self) -> usize {
        self.gap_start
    }

    pub fn len(&self) -> usize {
        self.gap_start + (self.buffer.len() - self.gap_end)
    }
}
