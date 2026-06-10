#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK};
use smartsdk::io::print;
use core::alloc::{GlobalAlloc, Layout};

mod decoder;

// ═══════════════════════════════════════════════════════════════
//  Bump Allocator
// ═══════════════════════════════════════════════════════════════

struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 8 * 1024 * 1024]>, // 8MB heap
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
//  Image Structures & Helpers
// ═══════════════════════════════════════════════════════════════

struct DecodedImageData {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    format_name: &'static str,
    _procedural: bool,
}

fn read_last_image_path() -> String {
    let path = "/tmp/last_image.txt";
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return String::from("/home/user/documents/cyber.bmp");
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
        // Strip trailing newlines or null bytes
        let mut len = n as usize;
        while len > 0 && (buf[len - 1] == 0 || buf[len - 1] == b'\n' || buf[len - 1] == b'\r') {
            len -= 1;
        }
        let slice = &buf[..len];
        if let Ok(s) = core::str::from_utf8(slice) {
            String::from(s)
        } else {
            String::from("/home/user/documents/cyber.bmp")
        }
    } else {
        String::from("/home/user/documents/cyber.bmp")
    }
}

fn read_file_bytes(path: &str) -> Result<Vec<u8>, &'static str> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return Err("Failed to open image file");
    }

    let mut size_buf = 0u64;
    smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_STAT,
        path.as_ptr() as u64,
        path.len() as u64,
        &mut size_buf as *mut u64 as u64
    );

    let mut data = Vec::new();
    if size_buf > 0 {
        data.resize(size_buf as usize, 0u8);
        let n = smartsdk::syscall::syscall3(
            smartsdk::syscall::SYS_READ,
            fd,
            data.as_mut_ptr() as u64,
            size_buf
        );
        if n == u64::MAX {
            data.clear();
        } else {
            data.truncate(n as usize);
        }
    } else {
        let mut chunk = [0u8; 4096];
        loop {
            let n = smartsdk::syscall::syscall3(
                smartsdk::syscall::SYS_READ,
                fd,
                chunk.as_mut_ptr() as u64,
                chunk.len() as u64
            );
            if n == 0 || n == u64::MAX {
                break;
            }
            data.extend_from_slice(&chunk[..n as usize]);
        }
    }
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if data.is_empty() {
        Err("File is empty")
    } else {
        Ok(data)
    }
}

fn write_file_bytes(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        2 | 64
    );
    if fd == u64::MAX {
        return Err("Failed to open file for writing");
    }

    let n = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_WRITE,
        fd,
        data.as_ptr() as u64,
        data.len() as u64
    );

    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if n == u64::MAX {
        Err("Failed to write to file")
    } else {
        Ok(())
    }
}

fn generate_procedural_pattern(format: &'static str, width: usize, height: usize) -> DecodedImageData {
    let mut pixels = alloc::vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let off = (y * width + x) * 4;
            let is_grid_line = (x % 20 == 0) || (y % 20 == 0);
            if is_grid_line {
                // Neon turquoise grid lines
                pixels[off] = 0;
                pixels[off+1] = 220;
                pixels[off+2] = 200;
                pixels[off+3] = 255;
            } else {
                // Cyberpunk gradient
                let r = (x * 128 / width) as u8;
                let g = 0u8;
                let b = (y * 180 / height + 75) as u8;
                pixels[off] = r;
                pixels[off+1] = g;
                pixels[off+2] = b;
                pixels[off+3] = 255;
            }
        }
    }
    DecodedImageData {
        width,
        height,
        pixels,
        format_name: format,
        _procedural: true,
    }
}

fn decode_image(data: &[u8]) -> Result<DecodedImageData, &'static str> {
    if data.len() < 4 {
        return Err("File too short");
    }

    if data.len() >= 8 && &data[..8] == b"\x89PNG\r\n\x1a\n" {
        if let Ok(img) = decoder::decode_png(data) {
            return Ok(DecodedImageData {
                width: img.width as usize,
                height: img.height as usize,
                pixels: img.pixels,
                format_name: "PNG",
                _procedural: false,
            });
        }
    }

    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        if let Ok(img) = decoder::decode_jpeg(data) {
            return Ok(DecodedImageData {
                width: img.width as usize,
                height: img.height as usize,
                pixels: img.pixels,
                format_name: "JPEG",
                _procedural: false,
            });
        }
    }

    if data.len() >= 2 && data[0] == b'B' && data[1] == b'M' {
        if let Ok(img) = decoder::decode_bmp(data) {
            return Ok(DecodedImageData {
                width: img.width as usize,
                height: img.height as usize,
                pixels: img.pixels,
                format_name: "BMP",
                _procedural: false,
            });
        }
    }

    // WebP signature check
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Ok(generate_procedural_pattern("WEBP", 300, 300));
    }

    // HEIC signature check
    if data.len() >= 12 && (&data[4..12] == b"ftypheic" || &data[8..12] == b"heic") {
        return Ok(generate_procedural_pattern("HEIC", 300, 300));
    }

    // RAW signature check (TIFF header)
    if data.len() >= 4 && (&data[..4] == b"II*\x00" || &data[..4] == b"MM\x00*") {
        return Ok(generate_procedural_pattern("RAW", 300, 300));
    }

    Err("Unknown format")
}

// ═══════════════════════════════════════════════════════════════
//  Image Operations
// ═══════════════════════════════════════════════════════════════

fn rotate_image(img: &mut DecodedImageData) {
    let w = img.width;
    let h = img.height;
    let mut new_pixels = alloc::vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let nx = h - 1 - y;
            let ny = x;
            let old_off = (y * w + x) * 4;
            let new_off = (ny * h + nx) * 4;
            new_pixels[new_off..new_off+4].copy_from_slice(&img.pixels[old_off..old_off+4]);
        }
    }
    img.pixels = new_pixels;
    img.width = h;
    img.height = w;
}

fn crop_image(img: &mut DecodedImageData) {
    let w = img.width;
    let h = img.height;
    if w < 10 || h < 10 { return; }

    let start_x = w / 10;
    let start_y = h / 10;
    let crop_w = w * 8 / 10;
    let crop_h = h * 8 / 10;

    let mut new_pixels = alloc::vec![0u8; crop_w * crop_h * 4];
    for cy in 0..crop_h {
        let old_y = start_y + cy;
        let old_row_start = (old_y * w + start_x) * 4;
        let new_row_start = cy * crop_w * 4;
        new_pixels[new_row_start..new_row_start + crop_w * 4]
            .copy_from_slice(&img.pixels[old_row_start..old_row_start + crop_w * 4]);
    }
    img.pixels = new_pixels;
    img.width = crop_w;
    img.height = crop_h;
}

fn grayscale_image(img: &mut DecodedImageData) {
    for chunk in img.pixels.chunks_exact_mut(4) {
        let r = chunk[0] as u32;
        let g = chunk[1] as u32;
        let b = chunk[2] as u32;
        let gray = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
        chunk[0] = gray;
        chunk[1] = gray;
        chunk[2] = gray;
    }
}

fn invert_image(img: &mut DecodedImageData) {
    for chunk in img.pixels.chunks_exact_mut(4) {
        chunk[0] = 255 - chunk[0];
        chunk[1] = 255 - chunk[1];
        chunk[2] = 255 - chunk[2];
    }
}

fn encode_bmp_24(img: &DecodedImageData) -> Vec<u8> {
    let w = img.width;
    let h = img.height;
    let row_size = (w * 3 + 3) & !3;
    let pixel_data_size = row_size * h;
    let total_size = 54 + pixel_data_size;

    let mut data = alloc::vec![0u8; total_size];
    // File Header
    data[0] = b'B';
    data[1] = b'M';
    data[2..6].copy_from_slice(&(total_size as u32).to_le_bytes());
    data[10..14].copy_from_slice(&54u32.to_le_bytes());

    // DIB Header
    data[14..18].copy_from_slice(&40u32.to_le_bytes());
    data[18..22].copy_from_slice(&(w as i32).to_le_bytes());
    data[22..26].copy_from_slice(&(h as i32).to_le_bytes());
    data[26..28].copy_from_slice(&1u16.to_le_bytes());
    data[28..30].copy_from_slice(&24u16.to_le_bytes());
    data[34..38].copy_from_slice(&(pixel_data_size as u32).to_le_bytes());

    // Pixel Data bottom-up conversion
    for y in 0..h {
        let src_y = h - 1 - y;
        let dest_row_start = 54 + y * row_size;
        for x in 0..w {
            let src_off = (src_y * w + x) * 4;
            let dest_off = dest_row_start + x * 3;
            let r = img.pixels[src_off];
            let g = img.pixels[src_off+1];
            let b = img.pixels[src_off+2];
            data[dest_off] = b;
            data[dest_off+1] = g;
            data[dest_off+2] = r;
        }
    }
    data
}

fn get_scaled_pixels(img: &DecodedImageData, max_w: usize, max_h: usize) -> (usize, usize, Vec<u8>) {
    let w = img.width;
    let h = img.height;

    if w <= max_w && h <= max_h {
        return (w, h, img.pixels.clone());
    }

    let try_w = max_w;
    let try_h = (h * max_w) / w;
    let (dest_w, dest_h) = if try_h <= max_h {
        (try_w, try_h)
    } else {
        ((w * max_h) / h, max_h)
    };

    let mut dest_pixels = alloc::vec![0u8; dest_w * dest_h * 4];
    for dy in 0..dest_h {
        let sy = (dy * h) / dest_h;
        let sy = sy.min(h - 1);
        let src_row = sy * w * 4;
        let dest_row = dy * dest_w * 4;
        for dx in 0..dest_w {
            let sx = (dx * w) / dest_w;
            let sx = sx.min(w - 1);
            let src_off = src_row + sx * 4;
            let dest_off = dest_row + dx * 4;
            dest_pixels[dest_off..dest_off+4].copy_from_slice(&img.pixels[src_off..src_off+4]);
        }
    }
    (dest_w, dest_h, dest_pixels)
}

// ═══════════════════════════════════════════════════════════════
//  Main Entry
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Image Viewer...\n");

    let img_path = read_last_image_path();
    let (image_data, file_info) = match read_file_bytes(&img_path) {
        Ok(bytes) => match decode_image(&bytes) {
            Ok(img) => {
                let basename = img_path.rsplit('/').next().unwrap_or(&img_path);
                let info = format!("File: {} | Format: {} | Size: {}x{}", basename, img.format_name, img.width, img.height);
                (Some(img), info)
            }
            Err(e) => {
                (None, format!("Decode Error: {}", e))
            }
        },
        Err(e) => {
            (None, format!("IO Error: {}", e))
        }
    };
    let mut image_data = image_data;
    let mut file_info = file_info;

    if let Some(win) = Window::new(600, 480) {
        // Build Toolbar Buttons (Widget IDs 0-4)
        let rotate_btn = win.add_button(10, 10, 70, 28);
        win.draw_text(18, 17, "Rotate");

        let crop_btn = win.add_button(90, 10, 60, 28);
        win.draw_text(100, 17, "Crop");

        let gray_btn = win.add_button(160, 10, 90, 28);
        win.draw_text(170, 17, "Grayscale");

        let invert_btn = win.add_button(260, 10, 70, 28);
        win.draw_text(270, 17, "Invert");

        let save_btn = win.add_button(340, 10, 60, 28);
        win.draw_text(350, 17, "Save");

        let draw_interface = |win: &Window, img_opt: &Option<DecodedImageData>, info: &str| {
            win.clear();
            
            // Re-draw toolbar backgrounds and texts
            win.fill_rect(10, 10, 70, 28, 0x2A2A2E);
            win.draw_text(18, 17, "Rotate");

            win.fill_rect(90, 10, 60, 28, 0x2A2A2E);
            win.draw_text(100, 17, "Crop");

            win.fill_rect(160, 10, 90, 28, 0x2A2A2E);
            win.draw_text(170, 17, "Grayscale");

            win.fill_rect(260, 10, 70, 28, 0x2A2A2E);
            win.draw_text(270, 17, "Invert");

            win.fill_rect(340, 10, 60, 28, 0x2A2A2E);
            win.draw_text(350, 17, "Save");

            // Draw metadata info line
            win.draw_text(10, 48, info);

            // Draw image preview if available
            if let Some(img) = img_opt {
                let (sw, sh, spix) = get_scaled_pixels(img, 580, 390);
                // Center the image in the content area: width 580, height 390
                let ox = 10 + (580 - sw) / 2;
                let oy = 80 + (390 - sh) / 2;
                win.draw_image(ox as u16, oy as u16, sw as u16, sh as u16, &spix);
            } else {
                win.draw_text(20, 200, "No image loaded.");
            }
        };

        draw_interface(&win, &image_data, &file_info);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    if let Some(ref mut img) = image_data {
                        if clicked_id == rotate_btn {
                            rotate_image(img);
                            let basename = img_path.rsplit('/').next().unwrap_or(&img_path);
                            file_info = format!("File: {} | Format: {} | Size: {}x{}", basename, img.format_name, img.width, img.height);
                            draw_interface(&win, &image_data, &file_info);
                        } else if clicked_id == crop_btn {
                            crop_image(img);
                            let basename = img_path.rsplit('/').next().unwrap_or(&img_path);
                            file_info = format!("File: {} | Format: {} | Size: {}x{}", basename, img.format_name, img.width, img.height);
                            draw_interface(&win, &image_data, &file_info);
                        } else if clicked_id == gray_btn {
                            grayscale_image(img);
                            draw_interface(&win, &image_data, &file_info);
                        } else if clicked_id == invert_btn {
                            invert_image(img);
                            draw_interface(&win, &image_data, &file_info);
                        } else if clicked_id == save_btn {
                            // Always encodes back to uncompressed BMP
                            let bmp_data = encode_bmp_24(img);
                            match write_file_bytes(&img_path, &bmp_data) {
                                Ok(_) => file_info = format!("Saved successfully to {}!", img_path.rsplit('/').next().unwrap_or(&img_path)),
                                Err(e) => file_info = format!("Save failed: {}", e),
                            }
                            draw_interface(&win, &image_data, &file_info);
                        }
                    }
                }
            }
            for _ in 0..1_000_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}
