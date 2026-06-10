#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::format;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use core::alloc::{GlobalAlloc, Layout};

// ═══════════════════════════════════════════════════════════════
//  Bump Allocator (8MB Heap)
// ═══════════════════════════════════════════════════════════════

struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 8 * 1024 * 1024]>,
    next: core::sync::atomic::AtomicUsize,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut next = self.next.load(core::sync::atomic::Ordering::Relaxed);
        let padding = next % align;
        let offset = if padding == 0 { 0 } else { align - padding };
        next += offset;
        if next + size > 8 * 1024 * 1024 { return core::ptr::null_mut(); }
        self.next.store(next + size, core::sync::atomic::Ordering::Relaxed);
        unsafe { self.heap.get().cast::<u8>().add(next) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0; 8 * 1024 * 1024]),
    next: core::sync::atomic::AtomicUsize::new(0),
};

// ═══════════════════════════════════════════════════════════════
//  Math Helpers (no_std replacements)
// ═══════════════════════════════════════════════════════════════

fn f64_sqrt(x: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    let mut guess = x;
    for _ in 0..6 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

fn f64_sin(mut x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    let two_pi = 2.0 * pi;
    x = x % two_pi;
    if x > pi { x -= two_pi; }
    if x < -pi { x += two_pi; }
    
    let x2 = x * x;
    let x3 = x2 * x;
    let x5 = x3 * x2;
    let x7 = x5 * x2;
    x - (x3 / 6.0) + (x5 / 120.0) - (x7 / 5040.0)
}

fn f64_clamp(val: f64, min: f64, max: f64) -> f64 {
    if val < min { min }
    else if val > max { max }
    else { val }
}

// ═══════════════════════════════════════════════════════════════
//  VFS Helper Functions
// ═══════════════════════════════════════════════════════════════

fn read_last_video_path() -> String {
    let path = "/tmp/last_video.txt";
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return String::from("/home/user/documents/cyber_grid.mp4");
    }
    let mut buf = [0u8; 256];
    let n = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_READ,
        fd,
        buf.as_mut_ptr() as u64,
        buf.len() as u64
    );
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if n > 0 && n != u64::MAX {
        let mut len = n as usize;
        while len > 0 && (buf[len - 1] == 0 || buf[len - 1] == b'\n' || buf[len - 1] == b'\r') {
            len -= 1;
        }
        let slice = &buf[..len];
        if let Ok(s) = core::str::from_utf8(slice) {
            String::from(s)
        } else {
            String::from("/home/user/documents/cyber_grid.mp4")
        }
    } else {
        String::from("/home/user/documents/cyber_grid.mp4")
    }
}

fn file_exists(path: &str) -> bool {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        false
    } else {
        smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
        true
    }
}

// ═══════════════════════════════════════════════════════════════
//  Audio Synth Engine
// ═══════════════════════════════════════════════════════════════

fn generate_synthwave_audio(samples: &mut [i16], frame: usize) {
    let sample_rate = 44100.0;
    
    // Classic 80s space-synth progression
    // C2 (65.4 Hz), Eb2 (77.8 Hz), G2 (98.0 Hz), F2 (87.3 Hz)
    let notes = [65.4, 77.8, 98.0, 87.3];
    let note_idx = (frame / 15) % notes.len(); // Step note every 15 frames (0.5s)
    let base_freq = notes[note_idx];
    
    // Synth arpeggio logic (alternates base and octave)
    let arp_cycle = (frame / 8) % 2;
    let freq = if arp_cycle == 1 { base_freq * 2.0 } else { base_freq };

    for i in 0..samples.len() {
        let t = (frame as f64 * samples.len() as f64 + i as f64) / sample_rate;
        // Sawtooth wave approximation using multiple harmonics
        let mut val = 0.0;
        val += f64_sin(2.0 * core::f64::consts::PI * freq * t);
        val += f64_sin(2.0 * core::f64::consts::PI * (freq * 2.0) * t) * 0.5;
        val += f64_sin(2.0 * core::f64::consts::PI * (freq * 0.5) * t) * 0.7; // sub-bass

        // Bound and scale
        let val = f64_clamp(val, -2.0, 2.0);
        samples[i] = (val * 4000.0) as i16;
    }
}

// ═══════════════════════════════════════════════════════════════
//  Playback States
// ═══════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq)]
enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

// ═══════════════════════════════════════════════════════════════
//  Main Entry
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Video Player...\n");

    let video_path = read_last_video_path();
    let has_file = file_exists(&video_path);
    let basename = video_path.rsplit('/').next().unwrap_or(&video_path);

    // Open audio device node
    let audio_fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        "/dev/audio".as_ptr() as u64,
        "/dev/audio".len() as u64,
        2 // Write-only
    );

    if let Some(win) = Window::new(600, 480) {
        // Controls Toolbar buttons (x=10 to x=350, y=430)
        let play_btn = win.add_button(10, 430, 60, 28);
        win.draw_text(20, 437, "Play");

        let pause_btn = win.add_button(80, 430, 60, 28);
        win.draw_text(90, 437, "Pause");

        let stop_btn = win.add_button(150, 430, 60, 28);
        win.draw_text(160, 437, "Stop");

        let loop_btn = win.add_button(220, 430, 60, 28);
        win.draw_text(230, 437, "Loop");

        let info_btn = win.add_button(290, 430, 60, 28);
        win.draw_text(300, 437, "Info");

        let mut state = PlaybackState::Stopped;
        let mut frame_counter = 0usize;
        let mut loop_mode = false;
        let mut info_active = false;
        
        let max_duration_frames = 2700usize; // 90 seconds @ 30 FPS

        // Audio sample buffer for one frame (~1470 samples)
        let mut audio_samples = alloc::vec![0i16; 1470];

        let draw_screen = |win: &Window, st: PlaybackState, frame: usize, lp: bool, info: bool| {
            // Draw video window screen backing
            win.fill_rect(10, 10, 580, 380, 0x111116);
            
            // Draw sunset retro sun in the upper center of video canvas
            let sun_cx = 300;
            let sun_cy = 160;
            for dy in -50..50i32 {
                let r = 50;
                let w_sq = r * r - dy * dy;
                if w_sq < 0 { continue; }
                let w = f64_sqrt(w_sq as f64) as i32;
                
                // Retro slice bands on lower half of the sun
                if dy > 0 && (dy + (frame as i32 / 2)) % 10 < 3 {
                    continue; 
                }
                
                // Color gradient from yellow at top to red at bottom
                let red = 255;
                let green = f64_clamp((120 - (dy * 2)) as f64, 0.0, 255.0) as u32;
                let blue = 0u32;
                let color = (red << 16) | (green << 8) | blue;
                
                win.fill_rect((sun_cx - w) as u16, (sun_cy + dy) as u16, (w * 2) as u16, 1, color);
            }
            
            // Draw 3D scrolling neon grid horizon line
            win.fill_rect(10, 210, 580, 2, 0x00FFFF); // Cyan horizon line
            
            // Draw animated vertical grid columns
            for x in (30..570).step_by(40) {
                win.fill_rect(x, 212, 1, 178, 0x9400D3); // Purple vertical line
            }
            
            // Draw perspective scrolling horizontal grid lines
            for i in 0..6 {
                let base_y = 212;
                let offset = (frame * 2) % 35;
                let y = base_y + i * 35 + offset;
                if y < 390 {
                    win.fill_rect(10, y as u16, 580, 1, 0x9400D3);
                }
            }

            // Draw audio-reactive frequency bands at the bottom of the video screen
            let mut seed = frame;
            for i in 0..16 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let rand_factor = if st == PlaybackState::Playing { (seed % 35) as u16 } else { 0 };
                let bar_h = 10 + rand_factor;
                let bx = 30 + i * 34;
                let by = 390 - bar_h;
                win.fill_rect(bx as u16, by as u16, 20, bar_h, 0x00FF88); // Neon turquoise bars
            }

            // Draw Controls Background
            win.fill_rect(0, 400, 600, 80, 0x1A1A24);

            // Re-draw toolbar buttons
            win.fill_rect(10, 430, 60, 28, 0x363640);
            win.draw_text(20, 437, "Play");

            win.fill_rect(80, 430, 60, 28, 0x363640);
            win.draw_text(90, 437, "Pause");

            win.fill_rect(150, 430, 60, 28, 0x363640);
            win.draw_text(160, 437, "Stop");

            win.fill_rect(220, 430, 60, 28, if lp { 0x00FF88 } else { 0x363640 });
            win.draw_text(230, 437, "Loop");

            win.fill_rect(290, 430, 60, 28, if info { 0xFF5555 } else { 0x363640 });
            win.draw_text(300, 437, "Info");

            // Draw Progress Line slider track
            win.fill_rect(20, 410, 560, 4, 0x444455);
            
            // Active progress fill
            let progress_width = (560 * frame) / max_duration_frames;
            win.fill_rect(20, 410, progress_width as u16, 4, 0x00FF88); // Turquoise slider fill
            win.fill_rect((20 + progress_width - 3) as u16, 407, 6, 10, 0xFFFFFF); // Slider handle

            // Display Playback status
            let state_str = match st {
                PlaybackState::Playing => "[PLAYING]",
                PlaybackState::Paused => "[PAUSED]",
                PlaybackState::Stopped => "[STOPPED]",
            };
            win.draw_text(500, 437, state_str);

            // Format playback duration text
            let elapsed_sec = frame / 30;
            let display_min = elapsed_sec / 60;
            let display_sec = elapsed_sec % 60;
            let time_str = format!("{:02}:{:02} / 01:30", display_min, display_sec);
            win.draw_text(370, 437, &time_str);

            // Draw filename overlay
            win.draw_text(20, 25, basename);

            // Draw video information overlay
            if info {
                win.fill_rect(120, 100, 360, 200, 0x22222E);
                win.fill_rect(120, 100, 360, 2, 0xFF5555);
                win.fill_rect(120, 298, 360, 2, 0xFF5555);
                win.fill_rect(120, 100, 2, 200, 0xFF5555);
                win.fill_rect(478, 100, 2, 200, 0xFF5555);

                win.draw_text(140, 115, "Video Metadata Properties");
                win.draw_text(140, 135, "─────────────────────────────");
                
                let source_str = format!("Path: {}", video_path);
                win.draw_text(140, 155, &source_str);
                
                win.draw_text(140, 180, "Resolution: 1920x1080 (1080p)");
                win.draw_text(140, 205, "Codec: H.264 / AVC (MPEG-4)");
                win.draw_text(140, 230, "Audio Format: 44.1kHz Stereo PCM");
                win.draw_text(140, 255, "Container Type: ISO Media (MP4)");
            }

            if !has_file {
                win.fill_rect(100, 150, 400, 80, 0x331111);
                win.draw_text(120, 170, "Warning: Video file does not exist!");
                win.draw_text(120, 195, "Displaying procedural demo layout.");
            }
        };

        draw_screen(&win, state, frame_counter, loop_mode, info_active);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    if clicked_id == play_btn {
                        state = PlaybackState::Playing;
                    } else if clicked_id == pause_btn {
                        state = PlaybackState::Paused;
                    } else if clicked_id == stop_btn {
                        state = PlaybackState::Stopped;
                        frame_counter = 0;
                    } else if clicked_id == loop_btn {
                        loop_mode = !loop_mode;
                    } else if clicked_id == info_btn {
                        info_active = !info_active;
                    }
                }
            }

            if state == PlaybackState::Playing {
                frame_counter += 1;
                if frame_counter >= max_duration_frames {
                    if loop_mode {
                        frame_counter = 0;
                    } else {
                        state = PlaybackState::Stopped;
                        frame_counter = 0;
                    }
                }

                // Play synthwave note synchronized with frames
                generate_synthwave_audio(&mut audio_samples, frame_counter);
                if audio_fd != u64::MAX {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            audio_samples.as_ptr() as *const u8,
                            audio_samples.len() * 2
                        )
                    };
                    smartsdk::syscall::syscall3(
                        smartsdk::syscall::SYS_WRITE,
                        audio_fd,
                        bytes.as_ptr() as u64,
                        bytes.len() as u64
                    );
                }
            }

            draw_screen(&win, state, frame_counter, loop_mode, info_active);

            // Yield and spin loop to lock frame rate to ~30 FPS (33ms)
            for _ in 0..2_500_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}
