/// Animation primitives for Smart OS GUI.
///
/// Provides lerp, easing functions, and a simple Animation struct
/// for smooth transitions.

use super::theme::Color;

/// Linear interpolation between two f32 values.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Linear interpolation between two i32 values.
#[inline]
pub fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t.clamp(0.0, 1.0)) as i32
}

/// Color interpolation.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::rgb(
        lerp(a.r as f32, b.r as f32, t) as u8,
        lerp(a.g as f32, b.g as f32, t) as u8,
        lerp(a.b as f32, b.b as f32, t) as u8,
    )
}

// ── Easing functions (t: 0.0..1.0 → 0.0..1.0) ──

/// Ease-in quadratic: slow start.
#[inline]
pub fn ease_in_quad(t: f32) -> f32 { t * t }

/// Ease-out quadratic: slow end.
#[inline]
pub fn ease_out_quad(t: f32) -> f32 { t * (2.0 - t) }

/// Ease-in-out quadratic: slow start and end.
#[inline]
pub fn ease_in_out_quad(t: f32) -> f32 {
    if t < 0.5 { 2.0 * t * t } else { -1.0 + (4.0 - 2.0 * t) * t }
}

/// Ease-out cubic: decelerating.
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t1 = t - 1.0;
    t1 * t1 * t1 + 1.0
}

/// Ease-out elastic: bouncy overshoot.
pub fn ease_out_elastic(t: f32) -> f32 {
    if t <= 0.0 { return 0.0; }
    if t >= 1.0 { return 1.0; }
    // Simplified elastic using quadratic approximation
    let p = 0.3;
    let _s = p / 4.0;
    let t1 = t - 1.0;
    // Approximate 2^(-10*t) * sin using polynomial
    let decay = 1.0 - t1 * t1 * 10.0; // rough approximation
    1.0 - decay.abs() * (1.0 - t) * 0.3
}

/// A simple animation that interpolates between two values over time.
pub struct Animation {
    pub start_val: f32,
    pub end_val: f32,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub easing: fn(f32) -> f32,
    pub done: bool,
}

impl Animation {
    /// Create a new animation.
    pub fn new(from: f32, to: f32, duration_ticks: u64, easing: fn(f32) -> f32) -> Self {
        let start_tick = crate::drivers::timer::ticks();
        Self {
            start_val: from,
            end_val: to,
            start_tick,
            duration_ticks,
            easing,
            done: false,
        }
    }

    /// Get the current animated value.
    pub fn current_value(&mut self, current_tick: u64) -> f32 {
        if self.done {
            return self.end_val;
        }

        let elapsed = current_tick.saturating_sub(self.start_tick);
        if elapsed >= self.duration_ticks {
            self.done = true;
            return self.end_val;
        }

        let t = elapsed as f32 / self.duration_ticks as f32;
        let eased = (self.easing)(t);
        lerp(self.start_val, self.end_val, eased)
    }

    /// Check if the animation is complete.
    pub fn is_done(&self) -> bool {
        self.done
    }
}
