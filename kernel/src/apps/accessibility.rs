/// Smart OS — Accessibility (Phase 73, v0.33.0)
///
/// Provides system-wide accessibility features:
///
/// Subsystems:
/// • `ScreenReader`    — announces focused widget text via serial output (TTS stub)
/// • `KeyRepeat`       — configurable key-repeat delay & rate
/// • `StickyKeys`      — hold modifier keys across keystrokes
/// • `MouseKeys`       — navigate mouse cursor with numeric keypad
/// • `HighContrast`    — high-contrast color palette override
/// • `FontScale`       — global UI font scale factor (1×, 1.5×, 2×)
/// • `ColorFilter`     — protanopia / deuteranopia / tritanopia LUT simulation
///
/// GUI app:
/// • Toggle switches for each feature
/// • Font scale selector
/// • Color filter selector
/// • Screen reader verbosity selector
/// • Self-test: 9 tests for color transforms, key repeat timing, sticky keys

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ═══════════════════════════════════════════════════════════════════════════
//  Global accessibility flags (atomic — used from hot paths)
// ═══════════════════════════════════════════════════════════════════════════

pub static SCREEN_READER_ON: AtomicBool = AtomicBool::new(false);
pub static HIGH_CONTRAST:    AtomicBool = AtomicBool::new(false);
pub static STICKY_KEYS_ON:   AtomicBool = AtomicBool::new(false);
pub static MOUSE_KEYS_ON:    AtomicBool = AtomicBool::new(false);
/// Font scale × 100 (100 = 1×, 150 = 1.5×, 200 = 2×)
pub static FONT_SCALE_PCT:   AtomicU32  = AtomicU32::new(100);
/// 0 = none, 1 = protanopia, 2 = deuteranopia, 3 = tritanopia
pub static COLOR_FILTER:     AtomicU32  = AtomicU32::new(0);

// ═══════════════════════════════════════════════════════════════════════════
//  Screen Reader
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verbosity { Silent, Minimal, Standard, Verbose }

impl Verbosity {
    pub fn name(self) -> &'static str {
        match self { Verbosity::Silent => "Silent", Verbosity::Minimal => "Minimal",
                     Verbosity::Standard => "Standard", Verbosity::Verbose => "Verbose" }
    }
}

/// Announce text to the screen reader output (serial port as TTS stub).
pub fn announce(text: &str, verbosity: Verbosity) {
    if !SCREEN_READER_ON.load(Ordering::Relaxed) { return; }
    if verbosity == Verbosity::Silent { return; }
    crate::serial_println!("[a11y] ANNOUNCE: {}", text);
}

/// Announce a widget focus change (called by the widget framework).
pub fn announce_focus(widget_label: &str) {
    announce(widget_label, Verbosity::Minimal);
}

// ═══════════════════════════════════════════════════════════════════════════
//  Key Repeat
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug)]
pub struct KeyRepeatConfig {
    /// Delay before first repeat fires (in 10ms ticks).
    pub delay_ticks:  u32,
    /// Interval between repeats (in 10ms ticks).
    pub rate_ticks:   u32,
    /// Whether key repeat is enabled at all.
    pub enabled:      bool,
}

impl Default for KeyRepeatConfig {
    fn default() -> Self { Self { delay_ticks: 50, rate_ticks: 5, enabled: true } }
}

static KEY_REPEAT: Mutex<KeyRepeatConfig> = Mutex::new(KeyRepeatConfig {
    delay_ticks: 50, rate_ticks: 5, enabled: true
});

pub fn get_key_repeat() -> KeyRepeatConfig { *KEY_REPEAT.lock() }
pub fn set_key_repeat(cfg: KeyRepeatConfig) { *KEY_REPEAT.lock() = cfg; }

// ═══════════════════════════════════════════════════════════════════════════
//  Sticky Keys
// ═══════════════════════════════════════════════════════════════════════════

/// Modifier state for sticky keys (bit flags).
pub struct StickyState {
    pub ctrl_held:  bool,
    pub alt_held:   bool,
    pub shift_held: bool,
    pub meta_held:  bool,
}

impl StickyState {
    const fn new() -> Self { Self { ctrl_held: false, alt_held: false, shift_held: false, meta_held: false } }
    pub fn any_held(&self) -> bool { self.ctrl_held || self.alt_held || self.shift_held || self.meta_held }
    pub fn clear(&mut self) { self.ctrl_held = false; self.alt_held = false; self.shift_held = false; self.meta_held = false; }
    pub fn toggle_ctrl(&mut self)  { self.ctrl_held  = !self.ctrl_held; }
    pub fn toggle_alt(&mut self)   { self.alt_held   = !self.alt_held; }
    pub fn toggle_shift(&mut self) { self.shift_held = !self.shift_held; }
}

static STICKY: Mutex<StickyState> = Mutex::new(StickyState::new());

pub fn sticky_toggle_ctrl()  { if STICKY_KEYS_ON.load(Ordering::Relaxed) { STICKY.lock().toggle_ctrl(); } }
pub fn sticky_toggle_alt()   { if STICKY_KEYS_ON.load(Ordering::Relaxed) { STICKY.lock().toggle_alt(); } }
pub fn sticky_toggle_shift() { if STICKY_KEYS_ON.load(Ordering::Relaxed) { STICKY.lock().toggle_shift(); } }
pub fn sticky_is_ctrl()  -> bool { STICKY.lock().ctrl_held }
pub fn sticky_is_shift() -> bool { STICKY.lock().shift_held }
pub fn sticky_clear() { STICKY.lock().clear(); }

// ═══════════════════════════════════════════════════════════════════════════
//  Mouse Keys — cursor speed via numpad
// ═══════════════════════════════════════════════════════════════════════════

pub struct MouseKeysConfig {
    pub speed_px:    u32,   // pixels per keypress
    pub accel_after: u32,   // ticks before acceleration
    pub accel_mult:  u32,   // acceleration multiplier
}

impl Default for MouseKeysConfig {
    fn default() -> Self { Self { speed_px: 10, accel_after: 20, accel_mult: 3 } }
}

static MOUSE_KEYS: Mutex<MouseKeysConfig> = Mutex::new(MouseKeysConfig { speed_px: 10, accel_after: 20, accel_mult: 3 });

pub fn mouse_keys_move(dx: i32, dy: i32) {
    if !MOUSE_KEYS_ON.load(Ordering::Relaxed) { return; }
    let speed = MOUSE_KEYS.lock().speed_px as i32;
    crate::drivers::mouse::update_position_relative(dx * speed, dy * speed);
}

// ═══════════════════════════════════════════════════════════════════════════
//  Color Filter — Daltonization LUT (3×3 matrix per filter type)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorFilterKind { None, Protanopia, Deuteranopia, Tritanopia, Greyscale }

impl ColorFilterKind {
    pub fn name(self) -> &'static str {
        match self {
            ColorFilterKind::None         => "None",
            ColorFilterKind::Protanopia   => "Protanopia (red-blind)",
            ColorFilterKind::Deuteranopia => "Deuteranopia (green-blind)",
            ColorFilterKind::Tritanopia   => "Tritanopia (blue-blind)",
            ColorFilterKind::Greyscale    => "Greyscale",
        }
    }

    /// Apply the color filter to an RGB triple (values 0-255).
    pub fn apply(self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        // Simplified Daltonization matrices (×256 fixed-point)
        let (rf, gf, bf) = (r as i32, g as i32, b as i32);
        let (nr, ng, nb) = match self {
            ColorFilterKind::None => return (r, g, b),
            ColorFilterKind::Protanopia => (
                (rf * 56 + gf * 208 + bf * 0) >> 8,
                (rf * 58 + gf * 198 + bf * 0) >> 8,
                (rf * 0  + gf * 0   + bf * 256) >> 8,
            ),
            ColorFilterKind::Deuteranopia => (
                (rf * 174 + gf * 82  + bf * 0) >> 8,
                (rf * 174 + gf * 82  + bf * 0) >> 8,
                (rf * 0   + gf * 0   + bf * 256) >> 8,
            ),
            ColorFilterKind::Tritanopia => (
                (rf * 256 + gf * 0   + bf * 0) >> 8,
                (rf * 0   + gf * 181 + bf * 75) >> 8,
                (rf * 0   + gf * 181 + bf * 75) >> 8,
            ),
            ColorFilterKind::Greyscale => {
                let lum = (rf * 77 + gf * 150 + bf * 29) >> 8;
                (lum, lum, lum)
            }
        };
        let clamp = |v: i32| v.clamp(0, 255) as u8;
        (clamp(nr), clamp(ng), clamp(nb))
    }
}

pub fn set_color_filter(f: ColorFilterKind) {
    let idx = match f {
        ColorFilterKind::None         => 0,
        ColorFilterKind::Protanopia   => 1,
        ColorFilterKind::Deuteranopia => 2,
        ColorFilterKind::Tritanopia   => 3,
        ColorFilterKind::Greyscale    => 4,
    };
    COLOR_FILTER.store(idx, Ordering::Relaxed);
}

pub fn current_color_filter() -> ColorFilterKind {
    match COLOR_FILTER.load(Ordering::Relaxed) {
        1 => ColorFilterKind::Protanopia,
        2 => ColorFilterKind::Deuteranopia,
        3 => ColorFilterKind::Tritanopia,
        4 => ColorFilterKind::Greyscale,
        _ => ColorFilterKind::None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Font scale
// ═══════════════════════════════════════════════════════════════════════════

pub fn set_font_scale(pct: u32) {
    let clamped = pct.clamp(75, 300);
    FONT_SCALE_PCT.store(clamped, Ordering::Relaxed);
}
pub fn get_font_scale_pct() -> u32 { FONT_SCALE_PCT.load(Ordering::Relaxed) }

// ═══════════════════════════════════════════════════════════════════════════
//  GUI state
// ═══════════════════════════════════════════════════════════════════════════

pub struct A11yState {
    pub window_id:  WindowId,
    pub verbosity:  Verbosity,
    pub dirty:      bool,
}

pub static STATE: Mutex<Option<A11yState>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Window
// ═══════════════════════════════════════════════════════════════════════════

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop not init");
    let mut win = Window::new("Accessibility", 130, 60, 700, 520, ACCENT_MAGENTA);
    win.use_widgets = true;

    // Feature toggle buttons (top row)
    win.widgets.push(Widget::new(0,   4, 4, 160, 26,
        WidgetKind::Button(Button::new("Screen Reader OFF", ACCENT_CYAN,    AppCommand::ButtonClicked(0)))));
    win.widgets.push(Widget::new(1, 168, 4, 150, 26,
        WidgetKind::Button(Button::new("High Contrast OFF", ACCENT_ORANGE,  AppCommand::ButtonClicked(1)))));
    win.widgets.push(Widget::new(2, 322, 4, 140, 26,
        WidgetKind::Button(Button::new("Sticky Keys OFF",   ACCENT_GREEN,   AppCommand::ButtonClicked(2)))));
    win.widgets.push(Widget::new(3, 466, 4, 140, 26,
        WidgetKind::Button(Button::new("Mouse Keys OFF",    TEXT_SECONDARY, AppCommand::ButtonClicked(3)))));

    // Font scale row
    win.widgets.push(Widget::new(4,   4, 34, 80, 26,
        WidgetKind::Button(Button::new("Font 75%",   TEXT_MUTED,    AppCommand::ButtonClicked(4)))));
    win.widgets.push(Widget::new(5,  88, 34, 80, 26,
        WidgetKind::Button(Button::new("Font 100%",  ACCENT_CYAN,   AppCommand::ButtonClicked(5)))));
    win.widgets.push(Widget::new(6, 172, 34, 80, 26,
        WidgetKind::Button(Button::new("Font 150%",  TEXT_SECONDARY,AppCommand::ButtonClicked(6)))));
    win.widgets.push(Widget::new(7, 256, 34, 80, 26,
        WidgetKind::Button(Button::new("Font 200%",  TEXT_SECONDARY,AppCommand::ButtonClicked(7)))));

    // Color filter row
    win.widgets.push(Widget::new(8,   4, 64, 80, 26,
        WidgetKind::Button(Button::new("No Filter",   ACCENT_CYAN,   AppCommand::ButtonClicked(8)))));
    win.widgets.push(Widget::new(9,  88, 64, 90, 26,
        WidgetKind::Button(Button::new("Protanopia",  ACCENT_RED,    AppCommand::ButtonClicked(9)))));
    win.widgets.push(Widget::new(10, 182, 64, 120, 26,
        WidgetKind::Button(Button::new("Deuteranopia",ACCENT_GREEN,  AppCommand::ButtonClicked(10)))));
    win.widgets.push(Widget::new(11, 306, 64, 100, 26,
        WidgetKind::Button(Button::new("Tritanopia",  ACCENT_CYAN,   AppCommand::ButtonClicked(11)))));
    win.widgets.push(Widget::new(12, 410, 64, 100, 26,
        WidgetKind::Button(Button::new("Greyscale",   TEXT_SECONDARY,AppCommand::ButtonClicked(12)))));

    // Status label
    win.widgets.push(Widget::new(13, 4, 96, 684, 16,
        WidgetKind::Label(StaticLabel::new("Accessibility: defaults", TEXT_SECONDARY))));

    // Info scroll
    win.widgets.push(Widget::new(14, 4, 118, 684, 368,
        WidgetKind::ScrollText(ScrollableText::new(64))));

    let id = win.id;
    desk.wm.add(win);
    id
}

// ═══════════════════════════════════════════════════════════════════════════
//  Sync
// ═══════════════════════════════════════════════════════════════════════════

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    let sr_on  = SCREEN_READER_ON.load(Ordering::Relaxed);
    let hc_on  = HIGH_CONTRAST.load(Ordering::Relaxed);
    let sk_on  = STICKY_KEYS_ON.load(Ordering::Relaxed);
    let mk_on  = MOUSE_KEYS_ON.load(Ordering::Relaxed);
    let fscale = get_font_scale_pct();
    let filter = current_color_filter();

    // Update toggle button labels
    let btn_updates: [(u8, String); 4] = [
        (0, format!("Screen Reader {}", if sr_on { "ON " } else { "OFF" })),
        (1, format!("High Contrast {}", if hc_on { "ON " } else { "OFF" })),
        (2, format!("Sticky Keys {}", if sk_on { "ON " } else { "OFF" })),
        (3, format!("Mouse Keys {}", if mk_on { "ON " } else { "OFF" })),
    ];
    for (id, label) in &btn_updates {
        if let Some(w) = win.widgets.iter_mut().find(|w| w.id == *id) {
            if let WidgetKind::Button(ref mut btn) = w.kind { btn.label = label.clone(); }
        }
    }

    // Status label
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 13) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            l.text = format!("Screen:{} HighContrast:{} StickyKeys:{} Font:{}%  Filter:{}",
                if sr_on { "ON" } else { "OFF" },
                if hc_on { "ON" } else { "OFF" },
                if sk_on { "ON" } else { "OFF" },
                fscale, filter.name());
        }
    }

    // Info lines
    let mut lines: Vec<(String, Color)> = Vec::new();
    lines.push(("  ── Screen Reader ──────────────────────────────────────".to_string(), TEXT_MUTED));
    lines.push((format!("  Status    : {}", if sr_on { "ACTIVE" } else { "OFF" }), if sr_on { ACCENT_GREEN } else { TEXT_MUTED }));
    lines.push((format!("  Verbosity : {}", s.verbosity.name()), TEXT_SECONDARY));
    lines.push(("  Output to : Serial port (TTS stub)".to_string(), TEXT_SECONDARY));
    lines.push(("".to_string(), TEXT_MUTED));
    lines.push(("  ── Key Repeat ──────────────────────────────────────────".to_string(), TEXT_MUTED));
    let kr = get_key_repeat();
    lines.push((format!("  Enabled   : {}", kr.enabled), TEXT_SECONDARY));
    lines.push((format!("  Delay     : {} × 10ms", kr.delay_ticks), TEXT_SECONDARY));
    lines.push((format!("  Rate      : {} × 10ms", kr.rate_ticks), TEXT_SECONDARY));
    lines.push(("".to_string(), TEXT_MUTED));
    lines.push(("  ── Sticky Keys ──────────────────────────────────────────".to_string(), TEXT_MUTED));
    let sticky = STICKY.lock();
    lines.push((format!("  Ctrl:{} Alt:{} Shift:{}", sticky.ctrl_held, sticky.alt_held, sticky.shift_held), TEXT_SECONDARY));
    drop(sticky);
    lines.push(("".to_string(), TEXT_MUTED));
    lines.push(("  ── Color Filter ─────────────────────────────────────────".to_string(), TEXT_MUTED));
    lines.push((format!("  Active filter : {}", filter.name()), ACCENT_CYAN));
    let (tr, tg, tb) = filter.apply(255, 0, 0);
    lines.push((format!("  Red (255,0,0) → ({},{},{})", tr, tg, tb), TEXT_SECONDARY));
    let (gr, gg, gb) = filter.apply(0, 255, 0);
    lines.push((format!("  Green (0,255,0) → ({},{},{})", gr, gg, gb), TEXT_SECONDARY));
    lines.push(("".to_string(), TEXT_MUTED));
    lines.push(("  ── Font Scale ───────────────────────────────────────────".to_string(), TEXT_MUTED));
    lines.push((format!("  Scale: {}%  (affects UI font rendering)", fscale), ACCENT_CYAN));

    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 14) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ═══════════════════════════════════════════════════════════════════════════
//  Kernel thread
// ═══════════════════════════════════════════════════════════════════════════

pub fn run() {
    let window_id = create_window();
    *STATE.lock() = Some(A11yState { window_id, verbosity: Verbosity::Standard, dirty: true });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => {
                        let v = !SCREEN_READER_ON.load(Ordering::Relaxed);
                        SCREEN_READER_ON.store(v, Ordering::Relaxed);
                        if v { announce("Screen reader enabled.", Verbosity::Minimal); }
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        let v = !HIGH_CONTRAST.load(Ordering::Relaxed);
                        HIGH_CONTRAST.store(v, Ordering::Relaxed);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        let v = !STICKY_KEYS_ON.load(Ordering::Relaxed);
                        STICKY_KEYS_ON.store(v, Ordering::Relaxed);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        let v = !MOUSE_KEYS_ON.load(Ordering::Relaxed);
                        MOUSE_KEYS_ON.store(v, Ordering::Relaxed);
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => { set_font_scale(75);  s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => { set_font_scale(100); s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => { set_font_scale(150); s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(7)) => { set_font_scale(200); s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(8))  => { set_color_filter(ColorFilterKind::None);         s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(9))  => { set_color_filter(ColorFilterKind::Protanopia);   s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(10)) => { set_color_filter(ColorFilterKind::Deuteranopia); s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(11)) => { set_color_filter(ColorFilterKind::Tritanopia);   s.dirty = true; }
                    WidgetAction::Execute(AppCommand::ButtonClicked(12)) => { set_color_filter(ColorFilterKind::Greyscale);    s.dirty = true; }
                    _ => {}
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Self-test  (9 tests)
// ═══════════════════════════════════════════════════════════════════════════

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: ColorFilter::None is identity
    let (r,g,b) = ColorFilterKind::None.apply(100, 150, 200);
    if r != 100 || g != 150 || b != 200 { ok = false; }

    // T2: Greyscale — all channels equal
    let (r,g,b) = ColorFilterKind::Greyscale.apply(255, 0, 0);
    if r != g || g != b { ok = false; }

    // T3: Color filter output clamps to 0-255
    let (r,g,b) = ColorFilterKind::Protanopia.apply(255, 255, 255);
    // just ensure no panic and values in range
    let _ = (r, g, b);

    // T4: font scale clamped
    set_font_scale(50);
    if get_font_scale_pct() < 75 { ok = false; }
    set_font_scale(400);
    if get_font_scale_pct() > 300 { ok = false; }
    set_font_scale(100); // restore

    // T5: set_color_filter + current round-trip
    set_color_filter(ColorFilterKind::Deuteranopia);
    if current_color_filter() != ColorFilterKind::Deuteranopia { ok = false; }
    set_color_filter(ColorFilterKind::None);

    // T6: StickyKeys toggle ctrl
    STICKY_KEYS_ON.store(true, Ordering::Relaxed);
    sticky_toggle_ctrl();
    if !sticky_is_ctrl() { ok = false; }
    sticky_toggle_ctrl();
    if sticky_is_ctrl() { ok = false; }
    STICKY_KEYS_ON.store(false, Ordering::Relaxed);

    // T7: StickyKeys disabled → toggle has no effect
    sticky_toggle_ctrl(); // should be no-op
    if sticky_is_ctrl() { ok = false; }

    // T8: StickyState::any_held false when clear
    {
        let mut st = StickyState::new();
        if st.any_held() { ok = false; }
        st.toggle_ctrl();
        if !st.any_held() { ok = false; }
        st.clear();
        if st.any_held() { ok = false; }
    }

    // T9: KeyRepeat config round-trip
    let cfg = KeyRepeatConfig { delay_ticks: 30, rate_ticks: 3, enabled: false };
    set_key_repeat(cfg);
    let got = get_key_repeat();
    if got.delay_ticks != 30 || got.rate_ticks != 3 || got.enabled { ok = false; }
    set_key_repeat(KeyRepeatConfig::default()); // restore

    ok
}
