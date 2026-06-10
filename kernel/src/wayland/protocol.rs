/// Wayland wire protocol handler for Smart OS.
///
/// Handles opcodes for each core Wayland object type and generates
/// the correct response events. All multi-byte values are native-endian
/// (Wayland uses host byte order over Unix sockets).

use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;
use super::{WaylandClient, ObjectKind};

// ── Wire encoding helpers ─────────────────────────────────────────────────────

fn encode_header(buf: &mut Vec<u8>, sender: u32, opcode: u16, payload_len: usize) {
    let size = (8 + payload_len) as u32;
    buf.extend_from_slice(&sender.to_ne_bytes());
    buf.extend_from_slice(&((size << 16) | opcode as u32).to_ne_bytes());
}

fn encode_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

fn encode_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len() + 1; // include NUL
    encode_u32(buf, len as u32);
    buf.extend_from_slice(bytes);
    buf.push(0); // NUL terminator
    // Align to 4 bytes
    while buf.len() % 4 != 0 { buf.push(0); }
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    if off + 4 > data.len() { return 0; }
    u32::from_ne_bytes([data[off], data[off+1], data[off+2], data[off+3]])
}

fn read_str<'a>(data: &'a [u8], off: usize) -> (&'a str, usize) {
    let len = read_u32(data, off) as usize;
    if len == 0 || off + 4 + len > data.len() { return ("", off + 4); }
    let bytes = &data[off + 4 .. off + 4 + len - 1]; // strip NUL
    let s = core::str::from_utf8(bytes).unwrap_or("");
    let aligned = ((off + 4 + len) + 3) & !3;
    (s, aligned)
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Handle a Wayland message. Returns the response bytes to send back.
pub fn handle(
    client: &mut WaylandClient,
    object_id: u32,
    opcode: u16,
    payload: &[u8],
    kind: &ObjectKind,
) -> Vec<u8> {
    match kind {
        ObjectKind::Display    => handle_display(client, opcode, payload),
        ObjectKind::Registry   => handle_registry(client, opcode, payload),
        ObjectKind::Compositor => handle_compositor(client, opcode, payload),
        ObjectKind::Shm        => handle_shm(client, opcode, payload, object_id),
        ObjectKind::ShmPool { size } => handle_shm_pool(client, opcode, payload, object_id, *size),
        ObjectKind::Surface { .. }   => handle_surface(client, opcode, payload, object_id),
        ObjectKind::Buffer { .. }    => Vec::new(),
        ObjectKind::Seat       => handle_seat(client, opcode, payload, object_id),
        ObjectKind::Output     => Vec::new(),
        ObjectKind::XdgWmBase  => handle_xdg_wm_base(client, opcode, payload, object_id),
        ObjectKind::XdgSurface { surface_id } => {
            handle_xdg_surface(client, opcode, payload, object_id, *surface_id)
        }
        ObjectKind::XdgToplevel { title, surface_id } => {
            handle_xdg_toplevel(client, opcode, payload, object_id,
                title.clone(), *surface_id)
        }
        ObjectKind::Callback   => Vec::new(),
        _ => {
            crate::serial_println!("[wayland] unknown object {} opcode {}", object_id, opcode);
            Vec::new()
        }
    }
}

// ── wl_display ────────────────────────────────────────────────────────────────
// Opcodes: 0=sync, 1=get_registry

fn handle_display(client: &mut WaylandClient, opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        0 => {
            // sync(callback_id) → wl_callback::done
            let cb_id = read_u32(payload, 0);
            client.insert(cb_id, ObjectKind::Callback);
            let serial = client.serial();
            // wl_callback::done event (opcode 0)
            encode_header(&mut out, cb_id, 0, 4);
            encode_u32(&mut out, serial);
            // wl_display::delete_id (opcode 1) to release the callback object
            encode_header(&mut out, 1, 1, 4);
            encode_u32(&mut out, cb_id);
        }
        1 => {
            // get_registry(new_id)
            let reg_id = read_u32(payload, 0);
            client.insert(reg_id, ObjectKind::Registry);
            // Advertise all globals
            out.extend(advertise_globals(reg_id));
        }
        _ => {}
    }
    out
}

/// Send wl_registry::global events for all supported interfaces.
fn advertise_globals(registry_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let globals: &[(&str, u32, u32)] = &[
        ("wl_compositor", 3, 5),
        ("wl_shm",        4, 1),
        ("wl_seat",       5, 7),
        ("wl_output",     6, 4),
        ("xdg_wm_base",   7, 5),
    ];
    for (i, (iface, obj_id, ver)) in globals.iter().enumerate() {
        // wl_registry::global (opcode 0): name(u32) interface(str) version(u32)
        let mut payload = Vec::new();
        encode_u32(&mut payload, (i + 1) as u32);  // global name
        encode_str(&mut payload, iface);
        encode_u32(&mut payload, *ver);
        encode_header(&mut out, registry_id, 0, payload.len());
        out.extend(payload);
        let _ = obj_id; // suppress warning
    }
    out
}

// ── wl_registry ───────────────────────────────────────────────────────────────
// Opcodes: 0=bind

fn handle_registry(client: &mut WaylandClient, opcode: u16, payload: &[u8]) -> Vec<u8> {
    if opcode != 0 { return Vec::new(); } // only `bind` exists
    // bind(name, interface_str, version, new_id)
    let name       = read_u32(payload, 0);
    let (iface, off) = read_str(payload, 4);
    let _version   = read_u32(payload, off);
    let new_id     = read_u32(payload, off + 4);

    let kind = match (name, iface) {
        (1, _) | (_, "wl_compositor") => ObjectKind::Compositor,
        (2, _) | (_, "wl_shm")        => ObjectKind::Shm,
        (3, _) | (_, "wl_seat")       => ObjectKind::Seat,
        (4, _) | (_, "wl_output")     => {
            // Emit wl_output::geometry + mode events after binding
            let mut out = Vec::new();
            client.insert(new_id, ObjectKind::Output);
            out.extend(send_output_events(new_id));
            return out;
        }
        (5, _) | (_, "xdg_wm_base")   => ObjectKind::XdgWmBase,
        _                              => ObjectKind::Unknown,
    };
    client.insert(new_id, kind);
    Vec::new()
}

fn send_output_events(output_id: u32) -> Vec<u8> {
    let (sw, sh) = super::screen_size();
    let mut out = Vec::new();
    // wl_output::geometry (opcode 0)
    let mut g = Vec::new();
    encode_u32(&mut g, 0); // x
    encode_u32(&mut g, 0); // y
    encode_u32(&mut g, sw * 25 / 96); // physical_width mm
    encode_u32(&mut g, sh * 25 / 96); // physical_height mm
    encode_u32(&mut g, 0); // subpixel UNKNOWN
    encode_str(&mut g, "Smart OS");
    encode_str(&mut g, "Virtual-1");
    encode_u32(&mut g, 0); // transform NORMAL
    encode_header(&mut out, output_id, 0, g.len());
    out.extend(g);
    // wl_output::mode (opcode 1): flags=3(current|preferred)
    let mut m = Vec::new();
    encode_u32(&mut m, 3);
    encode_u32(&mut m, sw);
    encode_u32(&mut m, sh);
    encode_u32(&mut m, 60_000); // 60 Hz in mHz
    encode_header(&mut out, output_id, 1, m.len());
    out.extend(m);
    // wl_output::done (opcode 2)
    encode_header(&mut out, output_id, 2, 0);
    out
}

// ── wl_compositor ─────────────────────────────────────────────────────────────
// Opcodes: 0=create_surface, 1=create_region

fn handle_compositor(client: &mut WaylandClient, opcode: u16, payload: &[u8]) -> Vec<u8> {
    match opcode {
        0 => {
            let surf_id = read_u32(payload, 0);
            client.insert(surf_id, ObjectKind::Surface { window_id: None });
        }
        1 => {
            // create_region — we don't track regions, just insert unknown
            let region_id = read_u32(payload, 0);
            client.insert(region_id, ObjectKind::Unknown);
        }
        _ => {}
    }
    Vec::new()
}

// ── wl_shm ────────────────────────────────────────────────────────────────────
// Opcodes: 0=create_pool

fn handle_shm(client: &mut WaylandClient, opcode: u16, payload: &[u8],
              _shm_id: u32) -> Vec<u8> {
    if opcode == 0 {
        // create_pool(new_id, fd, size)
        let pool_id = read_u32(payload, 0);
        let _fd     = read_u32(payload, 4);
        let size    = read_u32(payload, 8);
        client.insert(pool_id, ObjectKind::ShmPool { size });
    }
    Vec::new()
}

// ── wl_shm_pool ───────────────────────────────────────────────────────────────
// Opcodes: 0=create_buffer, 1=destroy, 2=resize

fn handle_shm_pool(client: &mut WaylandClient, opcode: u16, payload: &[u8],
                   pool_id: u32, _size: u32) -> Vec<u8> {
    match opcode {
        0 => {
            // create_buffer(new_id, offset, width, height, stride, format)
            let buf_id = read_u32(payload, 0);
            let _off   = read_u32(payload, 4);
            let width  = read_u32(payload, 8);
            let height = read_u32(payload, 12);
            let _stride = read_u32(payload, 16);
            let format = read_u32(payload, 20);
            client.insert(buf_id, ObjectKind::Buffer { width, height, format });
        }
        1 => { client.remove(pool_id); }
        2 => {
            let new_size = read_u32(payload, 0);
            if let Some(obj) = client.objects.get_mut(&pool_id) {
                obj.kind = ObjectKind::ShmPool { size: new_size };
            }
        }
        _ => {}
    }
    Vec::new()
}

// ── wl_surface ────────────────────────────────────────────────────────────────
// Opcodes: 0=destroy, 1=attach, 2=damage, 3=frame, 4=set_opaque_region,
//          5=set_input_region, 6=commit, 7=set_buffer_transform, 8=set_buffer_scale

fn handle_surface(client: &mut WaylandClient, opcode: u16, payload: &[u8],
                  surf_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        0 => {
            // destroy
            if let Some(ObjectKind::Surface { window_id: Some(wid) }) =
                client.objects.get(&surf_id).map(|o| &o.kind)
            {
                crate::gui::compositor::destroy_window(*wid);
            }
            client.remove(surf_id);
        }
        1 => {
            // attach(buffer_id, x, y) — associate a buffer with this surface
            let _buf = read_u32(payload, 0);
        }
        3 => {
            // frame(callback_id) — send done when next frame is rendered
            let cb_id = read_u32(payload, 0);
            client.insert(cb_id, ObjectKind::Callback);
            // Immediately signal done (we render synchronously)
            let serial = client.serial();
            encode_header(&mut out, cb_id, 0, 4);
            encode_u32(&mut out, serial);
            encode_header(&mut out, 1, 1, 4);
            encode_u32(&mut out, cb_id);
        }
        6 => {
            // commit — apply pending state: create or update the compositor window
            super::surface::commit(client, surf_id);
        }
        _ => {}
    }
    out
}

// ── wl_seat ───────────────────────────────────────────────────────────────────
// Opcodes: 0=get_pointer, 1=get_keyboard, 2=get_touch, 3=release

fn handle_seat(client: &mut WaylandClient, opcode: u16, payload: &[u8],
               seat_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        0 => {
            // get_pointer(new_id)
            let ptr_id = read_u32(payload, 0);
            client.insert(ptr_id, ObjectKind::Pointer);
        }
        1 => {
            // get_keyboard(new_id)
            let kbd_id = read_u32(payload, 0);
            client.insert(kbd_id, ObjectKind::Keyboard);
            // Send keymap event: no keymap (XKB_KEYMAP_FORMAT_NO_KEYMAP=0)
            let mut p = Vec::new();
            encode_u32(&mut p, 0); // format NO_KEYMAP
            encode_u32(&mut p, 0); // fd = 0 (invalid — stub)
            encode_u32(&mut p, 0); // size = 0
            encode_header(&mut out, kbd_id, 0, p.len());
            out.extend(p);
        }
        _ => { let _ = seat_id; }
    }
    // Also send wl_seat::capabilities (opcode 0) event: POINTER|KEYBOARD = 3
    let mut cap = Vec::new();
    encode_u32(&mut cap, 3);
    encode_header(&mut out, seat_id, 0, cap.len());
    out.extend(cap);
    out
}

// ── xdg_wm_base ──────────────────────────────────────────────────────────────
// Opcodes: 0=destroy, 1=create_positioner, 2=get_xdg_surface, 3=pong

fn handle_xdg_wm_base(client: &mut WaylandClient, opcode: u16, payload: &[u8],
                       wm_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        2 => {
            // get_xdg_surface(new_id, surface_id)
            let xdg_id  = read_u32(payload, 0);
            let surf_id = read_u32(payload, 4);
            client.insert(xdg_id, ObjectKind::XdgSurface { surface_id: surf_id });
        }
        3 => {} // pong — keep-alive reply
        _ => { let _ = wm_id; }
    }
    out
}

// ── xdg_surface ───────────────────────────────────────────────────────────────
// Opcodes: 0=destroy, 1=get_toplevel, 2=get_popup, 3=set_window_geometry, 4=ack_configure

fn handle_xdg_surface(client: &mut WaylandClient, opcode: u16, payload: &[u8],
                       xdg_id: u32, surface_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        1 => {
            // get_toplevel(new_id)
            let tl_id = read_u32(payload, 0);
            client.insert(tl_id, ObjectKind::XdgToplevel {
                title: String::new(),
                surface_id,
            });
            // Send xdg_surface::configure (opcode 0) → triggers ack_configure
            let serial = client.serial();
            encode_header(&mut out, xdg_id, 0, 4);
            encode_u32(&mut out, serial);
        }
        4 => {} // ack_configure — no response needed
        _ => { let _ = xdg_id; }
    }
    out
}

// ── xdg_toplevel ─────────────────────────────────────────────────────────────
// Opcodes: 0=destroy, 1=set_parent, 2=set_title, 3=set_app_id,
//          4=show_window_menu, 5=move, 6=resize, 7=set_max_size,
//          8=set_min_size, 9=set_maximized, 10=unset_maximized,
//          11=set_fullscreen, 12=unset_fullscreen, 13=set_minimized

fn handle_xdg_toplevel(client: &mut WaylandClient, opcode: u16, payload: &[u8],
                        tl_id: u32, title: String, surface_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    match opcode {
        2 => {
            // set_title(string)
            let (new_title, _) = read_str(payload, 0);
            if let Some(obj) = client.objects.get_mut(&tl_id) {
                obj.kind = ObjectKind::XdgToplevel {
                    title: new_title.to_string(),
                    surface_id,
                };
            }
            // Update the compositor window title
            super::surface::set_title(client, surface_id, new_title);
        }
        9 | 11 => {
            // maximize / fullscreen — send configure with maximized state
            let serial = client.serial();
            let (sw, sh) = super::screen_size();
            let mut p = Vec::new();
            encode_u32(&mut p, sw);
            encode_u32(&mut p, sh);
            // states array: maximized=2, fullscreen=3, activated=4
            let state: u32 = if opcode == 11 { 3 } else { 2 };
            encode_u32(&mut p, 4); // states array size (1 element × 4 bytes)
            encode_u32(&mut p, state);
            // xdg_toplevel::configure (opcode 0)
            encode_header(&mut out, tl_id, 0, p.len());
            out.extend(p);
            // follow with xdg_surface::configure — surface_id is in xdg_surface object
            // find the xdg_surface that owns this toplevel
            for (_, obj) in &client.objects {
                if let ObjectKind::XdgSurface { surface_id: sid } = obj.kind {
                    if sid == surface_id {
                        encode_header(&mut out, obj.id, 0, 4);
                        encode_u32(&mut out, serial);
                        break;
                    }
                }
            }
        }
        _ => { let _ = (tl_id, title, surface_id); }
    }
    out
}
