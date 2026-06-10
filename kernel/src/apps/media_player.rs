/// Phase 54: Media Player for Smart OS.
///
/// Features:
///   • WAV / raw PCM decoder  — parses RIFF headers (PCM, 8/16/32-bit)
///   • Playlist               — Vec<String> of VFS paths, wrapping navigation
///   • Playback state machine — Stopped / Playing / Paused
///   • Seek                   — sample-offset seek, formatted MM:SS display
///   • Waveform visualiser    — 32-column ASCII block peak display
///   • HDA MIXER submission   — pushes decoded i16 samples to SoftwareMixer
///   • GUI                    — window with transport, seek, waveform, playlist

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ─────────────────────────────────────────────────────────────────────────────
//  WAV / PCM decoder
// ─────────────────────────────────────────────────────────────────────────────

/// WAV file metadata extracted from the RIFF/WAVE header.
#[derive(Clone, Debug, Default)]
pub struct WavInfo {
    pub channels:        u16,
    pub sample_rate:     u32,
    pub bits_per_sample: u16,
    pub data_offset:     usize,
    pub data_len:        usize,
}

/// Parse a RIFF WAVE header.  Returns `None` for non-WAV files.
pub fn parse_wav(data: &[u8]) -> Option<WavInfo> {
    if data.len() < 44 { return None; }
    if &data[0..4] != b"RIFF" { return None; }
    if &data[8..12] != b"WAVE" { return None; }
    if &data[12..16] != b"fmt " { return None; }

    let audio_format    = u16::from_le_bytes([data[20], data[21]]);
    let channels        = u16::from_le_bytes([data[22], data[23]]);
    let sample_rate     = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);

    if audio_format != 1 { return None; } // PCM only

    // Scan for the "data" sub-chunk.
    let mut pos = 12usize;
    let mut data_offset = 0usize;
    let mut data_len    = 0usize;
    while pos + 8 <= data.len() {
        let tag = &data[pos..pos + 4];
        let sz  = u32::from_le_bytes([data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
        if tag == b"data" {
            data_offset = pos + 8;
            data_len    = sz.min(data.len().saturating_sub(data_offset));
            break;
        }
        pos += 8 + sz;
    }
    if data_offset == 0 { return None; }

    Some(WavInfo { channels, sample_rate, bits_per_sample, data_offset, data_len })
}

/// Decode PCM bytes into i16 samples.
/// Handles 8-bit (unsigned), 16-bit (signed LE), 32-bit (signed LE → downscale).
pub fn decode_pcm(data: &[u8], info: &WavInfo) -> Vec<i16> {
    let end = (info.data_offset + info.data_len).min(data.len());
    let pcm = &data[info.data_offset..end];
    let bps = (info.bits_per_sample / 8) as usize;
    if bps == 0 { return Vec::new(); }

    let n = pcm.len() / bps;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = i * bps;
        let s = match info.bits_per_sample {
            8  => ((pcm[off] as i16) - 128) << 8,
            16 => i16::from_le_bytes([pcm[off], pcm[off + 1]]),
            32 => {
                let v = i32::from_le_bytes([pcm[off], pcm[off+1], pcm[off+2], pcm[off+3]]);
                (v >> 16) as i16
            }
            _  => 0,
        };
        out.push(s);
    }
    out
}

/// Compute `columns` peak amplitude values from `samples` — each in [0, 8].
/// Used to draw an ASCII waveform.
pub fn waveform_peaks(samples: &[i16], columns: usize) -> Vec<u8> {
    if samples.is_empty() || columns == 0 {
        return alloc::vec![0u8; columns.max(1)];
    }
    let per_col = (samples.len() / columns).max(1);
    (0..columns).map(|c| {
        let start = c * per_col;
        let end   = ((c + 1) * per_col).min(samples.len());
        let peak  = samples[start..end].iter().map(|&s| s.unsigned_abs()).max().unwrap_or(0);
        ((peak as u32 * 8) / 32768) as u8
    }).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Playback state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState { Stopped, Playing, Paused }

pub struct PlayerState {
    pub window_id:  WindowId,
    pub playlist:   Vec<String>,
    pub current:    usize,
    pub play_state: PlayState,
    pub wav_info:   Option<WavInfo>,
    pub samples:    Vec<i16>,
    pub cursor:     usize,    // sample-index seek position
    pub volume:     u8,       // 0-100
    pub dirty:      bool,
}

pub static STATE: Mutex<Option<PlayerState>> = Mutex::new(None);

// ─────────────────────────────────────────────────────────────────────────────
//  Audio submission to HDA MIXER
// ─────────────────────────────────────────────────────────────────────────────

fn submit_audio(state: &mut PlayerState, n: usize) {
    if state.play_state != PlayState::Playing { return; }
    if state.cursor >= state.samples.len() {
        state.play_state = PlayState::Stopped;
        state.cursor = 0;
        state.dirty = true;
        return;
    }
    let end = (state.cursor + n).min(state.samples.len());
    let chunk = &state.samples[state.cursor..end];
    let count = chunk.len();

    if let Some(mut mixer) = crate::drivers::hda::MIXER.try_lock() {
        use alloc::collections::VecDeque;
        use crate::drivers::hda::AudioStream;
        if let Some(s) = mixer.streams.iter_mut().find(|s| s.id == 54) {
            s.volume = state.volume;
            for &sample in chunk { s.buffer.push_back(sample); }
        } else {
            let mut buf = VecDeque::with_capacity(count);
            for &sample in chunk { buf.push_back(sample); }
            mixer.streams.push(AudioStream { id: 54, buffer: buf, volume: state.volume });
        }
    }
    state.cursor += count;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Track management
// ─────────────────────────────────────────────────────────────────────────────

fn load_track(state: &mut PlayerState) {
    let path = match state.playlist.get(state.current).cloned() {
        Some(p) => p,
        None => { state.wav_info = None; state.samples.clear(); state.dirty = true; return; }
    };
    state.samples.clear();
    state.cursor = 0;
    state.wav_info = None;

    if let Ok(data) = crate::vfs::read_file_full(&path) {
        if let Some(info) = parse_wav(&data) {
            state.samples = decode_pcm(&data, &info);
            state.wav_info = Some(info);
        } else {
            // Raw 16-bit LE mono 44100 Hz fallback.
            state.samples = data.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            state.wav_info = Some(WavInfo {
                channels: 1, sample_rate: 44100, bits_per_sample: 16,
                data_offset: 0, data_len: data.len(),
            });
        }
    }
    state.dirty = true;
}

fn play_pause(state: &mut PlayerState) {
    state.play_state = match state.play_state {
        PlayState::Stopped => {
            if state.samples.is_empty() { load_track(state); }
            PlayState::Playing
        }
        PlayState::Playing => PlayState::Paused,
        PlayState::Paused  => PlayState::Playing,
    };
    state.dirty = true;
}

fn stop(state: &mut PlayerState) {
    state.play_state = PlayState::Stopped;
    state.cursor = 0;
    state.dirty = true;
}

fn prev_track(state: &mut PlayerState) {
    if !state.playlist.is_empty() {
        state.current = if state.current == 0 {
            state.playlist.len() - 1
        } else { state.current - 1 };
        load_track(state);
    }
}

fn next_track(state: &mut PlayerState) {
    if !state.playlist.is_empty() {
        state.current = (state.current + 1) % state.playlist.len();
        load_track(state);
        if state.play_state == PlayState::Playing { /* keep playing */ }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Display helpers
// ─────────────────────────────────────────────────────────────────────────────

pub fn now_playing_text(state: &PlayerState) -> String {
    let title = state.playlist.get(state.current)
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .unwrap_or_else(|| String::from("(no track)"));
    let sym = match state.play_state {
        PlayState::Playing => "[>]",
        PlayState::Paused  => "[=]",
        PlayState::Stopped => "[.]",
    };
    format!("{} {}", sym, title)
}

pub fn seek_text(state: &PlayerState) -> String {
    if state.samples.is_empty() { return String::from("0:00 / 0:00"); }
    let rate = state.wav_info.as_ref().map(|i| i.sample_rate as usize * i.channels as usize).unwrap_or(44100);
    let rate = rate.max(1);
    let cur_s = state.cursor / rate;
    let tot_s = state.samples.len() / rate;
    format!("{}:{:02} / {}:{:02}", cur_s / 60, cur_s % 60, tot_s / 60, tot_s % 60)
}

pub fn waveform_text(state: &PlayerState) -> String {
    const COLS: usize = 32;
    const CHARS: &[u8] = b" 1234567|";
    if state.samples.is_empty() {
        return (0..COLS).map(|_| ' ').collect();
    }
    let peaks = waveform_peaks(&state.samples, COLS);
    let mut s = String::with_capacity(COLS);
    for p in peaks {
        s.push(CHARS[(p as usize).min(8)] as char);
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
//  Widget IDs
// ─────────────────────────────────────────────────────────────────────────────

const BTN_PREV:  u8 = 1;
const BTN_PLAY:  u8 = 2;
const BTN_STOP:  u8 = 3;
const BTN_NEXT:  u8 = 4;
const BTN_VOLUP: u8 = 5;
const BTN_VOLDN: u8 = 6;
const LBL_NOW:   u8 = 10;
const LBL_SEEK:  u8 = 11;
const LBL_WAVE:  u8 = 12;
const LBL_VOL:   u8 = 13;
// Playlist items: 20..=27
const PL_BASE:   u8 = 20;
const PL_ROWS:   usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
//  GUI label update
// ─────────────────────────────────────────────────────────────────────────────

fn update_label(win: &mut Window, id: u8, text: String) {
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == id) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = text;
        }
    }
}

fn update_btn_label(win: &mut Window, id: u8, text: &str) {
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == id) {
        if let WidgetKind::Button(ref mut btn) = w.kind {
            btn.label = text.to_string();
        }
    }
}

fn sync_gui(state: &PlayerState, win: &mut Window) {
    update_label(win, LBL_NOW,  now_playing_text(state));
    update_label(win, LBL_SEEK, seek_text(state));
    update_label(win, LBL_WAVE, waveform_text(state));
    update_label(win, LBL_VOL,  format!("Vol {}%", state.volume));
    let play_lbl = match state.play_state { PlayState::Playing => "Pause", _ => "Play" };
    update_btn_label(win, BTN_PLAY, play_lbl);

    for i in 0..PL_ROWS {
        let wid = PL_BASE + i as u8;
        if let Some(w) = win.widgets.iter_mut().find(|w| w.id == wid) {
            if let WidgetKind::Button(ref mut btn) = w.kind {
                if let Some(path) = state.playlist.get(i) {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    let mark = if i == state.current { ">" } else { " " };
                    btn.label = format!("{} {}", mark, name);
                } else {
                    btn.label = String::new();
                }
            }
            w.visible = state.playlist.get(i).is_some();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Run (GUI app entry point)
// ─────────────────────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = Window::new("Media Player", 100, 80, 520, 480, ACCENT_CYAN);
        win.use_widgets = true;

        // Labels
        win.add_widget(Widget::new(LBL_NOW,  8, 8,   504, 20, WidgetKind::Label(StaticLabel::new("[.] (no track)", TEXT_PRIMARY))));
        win.add_widget(Widget::new(LBL_SEEK, 8, 32,  200, 18, WidgetKind::Label(StaticLabel::new("0:00 / 0:00", TEXT_SECONDARY))));
        win.add_widget(Widget::new(LBL_VOL,  8, 54,  100, 18, WidgetKind::Label(StaticLabel::new("Vol 80%", TEXT_SECONDARY))));
        win.add_widget(Widget::new(LBL_WAVE, 8, 76,  504, 18, WidgetKind::Label(StaticLabel::new("", TEXT_PRIMARY))));

        // Transport buttons
        win.add_widget(Widget::new(BTN_PREV,  8,   100, 60, 32, WidgetKind::Button(Button::new("|<", ACCENT_BLUE,  AppCommand::ButtonClicked(BTN_PREV)))));
        win.add_widget(Widget::new(BTN_PLAY,  74,  100, 80, 32, WidgetKind::Button(Button::new("Play", ACCENT_GREEN, AppCommand::ButtonClicked(BTN_PLAY)))));
        win.add_widget(Widget::new(BTN_STOP,  160, 100, 60, 32, WidgetKind::Button(Button::new("Stop", ACCENT_RED,   AppCommand::ButtonClicked(BTN_STOP)))));
        win.add_widget(Widget::new(BTN_NEXT,  226, 100, 60, 32, WidgetKind::Button(Button::new(">|", ACCENT_BLUE,  AppCommand::ButtonClicked(BTN_NEXT)))));
        win.add_widget(Widget::new(BTN_VOLUP, 310, 100, 40, 32, WidgetKind::Button(Button::new("+",  ACCENT_ORANGE, AppCommand::ButtonClicked(BTN_VOLUP)))));
        win.add_widget(Widget::new(BTN_VOLDN, 356, 100, 40, 32, WidgetKind::Button(Button::new("-",  ACCENT_ORANGE, AppCommand::ButtonClicked(BTN_VOLDN)))));

        // Playlist header label
        win.add_widget(Widget::new(99, 8, 140, 504, 16,
            WidgetKind::Label(StaticLabel::new("-- Playlist --", TEXT_SECONDARY))));

        // Playlist row buttons
        for i in 0..PL_ROWS {
            let wid = PL_BASE + i as u8;
            let y = 160 + i * 36;
            let mut w = Widget::new(wid, 8, y, 504, 30,
                WidgetKind::Button(Button::new("", ACCENT_BLUE, AppCommand::ButtonClicked(wid))));
            w.visible = false;
            win.add_widget(w);
        }

        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Pre-populate playlist from VFS.
    let playlist: Vec<String> = ["/media/test.wav", "/home/user/music.wav", "/data/sample.pcm"]
        .iter()
        .filter(|&&p| crate::vfs::stat(p).is_ok())
        .map(|&p| p.to_string())
        .collect();

    *STATE.lock() = Some(PlayerState {
        window_id, playlist, current: 0,
        play_state: PlayState::Stopped,
        wav_info: None, samples: Vec::new(), cursor: 0,
        volume: 80, dirty: true,
    });

    loop {
        // Poll for widget actions.
        let action = crate::gui::input::poll_action(window_id);

        let mut need_sync = false;
        {
            let mut guard = STATE.lock();
            let state = match guard.as_mut() { Some(s) => s, None => break };

            if let Some(WidgetAction::Execute(AppCommand::ButtonClicked(wid))) = action {
                match wid {
                    w if w == BTN_PLAY  => play_pause(state),
                    w if w == BTN_STOP  => stop(state),
                    w if w == BTN_PREV  => prev_track(state),
                    w if w == BTN_NEXT  => next_track(state),
                    w if w == BTN_VOLUP => { state.volume = (state.volume + 5).min(100); state.dirty = true; }
                    w if w == BTN_VOLDN => { state.volume = state.volume.saturating_sub(5); state.dirty = true; }
                    wid => {
                        let idx = wid.wrapping_sub(PL_BASE) as usize;
                        if idx < PL_ROWS && idx < state.playlist.len() {
                            state.current = idx;
                            load_track(state);
                            state.play_state = PlayState::Playing;
                        }
                    }
                }
            }

            // Audio DMA: push one chunk per tick.
            submit_audio(state, 1024);

            if state.dirty {
                state.dirty = false;
                need_sync = true;
            }
        }

        if need_sync {
            let mut desktop = DESKTOP.lock();
            if let Some(desk) = desktop.as_mut() {
                if let Some(win) = desk.wm.get_mut(window_id) {
                    if let Some(st) = STATE.lock().as_ref() {
                        sync_gui(st, win);
                        win.dirty = true;
                    }
                }
            }
        }

        crate::process::scheduler::yield_now();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // Test 1: parse_wav rejects short data.
    if parse_wav(&[0u8; 10]).is_some() {
        crate::serial_println!("[media-test] FAIL: short buffer parsed as WAV");
        ok = false;
    }

    // Test 2: parse_wav rejects non-WAV data.
    if parse_wav(&[0u8; 64]).is_some() {
        crate::serial_println!("[media-test] FAIL: zero buffer parsed as WAV");
        ok = false;
    }

    // Test 3: parse_wav accepts a minimal valid WAV.
    let wav: &[u8] = &[
        b'R',b'I',b'F',b'F', 0x24,0x00,0x00,0x00,
        b'W',b'A',b'V',b'E',
        b'f',b'm',b't',b' ', 0x10,0x00,0x00,0x00,
        0x01,0x00,            // PCM
        0x01,0x00,            // mono
        0x44,0xAC,0x00,0x00, // 44100 Hz
        0x88,0x58,0x01,0x00, // byte rate
        0x02,0x00,            // block align
        0x10,0x00,            // 16-bit
        b'd',b'a',b't',b'a', 0x04,0x00,0x00,0x00,
        0x00,0x40, 0x00,0xC0, // two i16 samples
    ];
    let info = match parse_wav(wav) {
        Some(i) => i,
        None => {
            crate::serial_println!("[media-test] FAIL: valid WAV rejected");
            ok = false;
            return ok;
        }
    };
    if info.channels != 1 || info.sample_rate != 44100 || info.bits_per_sample != 16 {
        crate::serial_println!("[media-test] FAIL: wrong WAV metadata");
        ok = false;
    }

    // Test 4: decode_pcm produces 2 samples.
    let samples = decode_pcm(wav, &info);
    if samples.len() != 2 {
        crate::serial_println!("[media-test] FAIL: expected 2 samples, got {}", samples.len());
        ok = false;
    }
    if samples.first() != Some(&0x4000i16) {
        crate::serial_println!("[media-test] FAIL: first sample should be 0x4000");
        ok = false;
    }

    // Test 5: waveform_peaks correct column count.
    let s: Vec<i16> = (0..256).map(|i| (i as i16) * 128).collect();
    let peaks = waveform_peaks(&s, 32);
    if peaks.len() != 32 { crate::serial_println!("[media-test] FAIL: wrong peak count"); ok = false; }

    // Test 6: peaks in range [0,8].
    if peaks.iter().any(|&p| p > 8) { crate::serial_println!("[media-test] FAIL: peak > 8"); ok = false; }

    // Test 7: silence → all zero peaks.
    let zeros = alloc::vec![0i16; 128];
    if waveform_peaks(&zeros, 16).iter().any(|&p| p != 0) {
        crate::serial_println!("[media-test] FAIL: silence should produce 0 peaks");
        ok = false;
    }

    // Test 8: empty samples → zero peaks.
    let ep = waveform_peaks(&[], 8);
    if ep.len() != 8 || ep.iter().any(|&p| p != 0) {
        crate::serial_println!("[media-test] FAIL: empty samples should give 8 zeros");
        ok = false;
    }

    // Test 9: seek_text formatting.
    // 44100 samples at 44100 Hz mono = 1 s; 88200 samples = 2 s.
    // seek_text format: "min:sec / min:sec" so 1 s → "0:01", 2 s → "0:02".
    let st = PlayerState {
        window_id: 0, playlist: Vec::new(), current: 0,
        play_state: PlayState::Stopped,
        wav_info: Some(WavInfo { channels: 1, sample_rate: 44100, bits_per_sample: 16, data_offset: 0, data_len: 0 }),
        samples: alloc::vec![0i16; 44100 * 2], cursor: 44100,
        volume: 80, dirty: false,
    };
    let t = seek_text(&st);
    if !t.contains("0:01") || !t.contains("0:02") {
        crate::serial_println!("[media-test] FAIL: seek text wrong: '{}'", t);
        ok = false;
    }

    // Test 10: now_playing_text contains filename.
    let st2 = PlayerState {
        window_id: 0,
        playlist: alloc::vec!["/music/song.wav".to_string()],
        current: 0, play_state: PlayState::Playing,
        wav_info: None, samples: Vec::new(), cursor: 0, volume: 80, dirty: false,
    };
    if !now_playing_text(&st2).contains("song.wav") {
        crate::serial_println!("[media-test] FAIL: now_playing should contain filename");
        ok = false;
    }

    if ok { crate::serial_println!("[media-test] All 10 media player tests PASSED"); }
    ok
}
