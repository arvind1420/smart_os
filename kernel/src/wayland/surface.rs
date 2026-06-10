/// wl_surface → gui::compositor window mapping for Smart OS Wayland bridge.

use alloc::string::String;
use super::{WaylandClient, ObjectKind};

/// Apply committed surface state: create or refresh a compositor window.
pub fn commit(client: &mut WaylandClient, surf_id: u32) {
    let title = find_toplevel_title(client, surf_id);

    let window_id: Option<u64> = match client.objects.get(&surf_id) {
        Some(obj) => {
            if let ObjectKind::Surface { window_id: Some(wid) } = obj.kind {
                // Window already exists — bring to front
                crate::gui::compositor::focus_window(wid);
                return;
            }
            None
        }
        None => return,
    };

    let _ = window_id;

    // Create a new compositor window for this surface
    let (sw, sh) = super::screen_size();
    let w = (sw * 3 / 4) as usize;
    let h = (sh * 3 / 4) as usize;
    let x = ((sw - w as u32) / 2) as usize;
    let y = ((sh - h as u32) / 2) as usize;

    let wid = crate::gui::compositor::create_window(
        &title.unwrap_or_else(|| String::from("Wayland App")),
        x, y, w, h,
    );

    // Store the window id back on the surface object
    if let Some(obj) = client.objects.get_mut(&surf_id) {
        obj.kind = ObjectKind::Surface { window_id: Some(wid) };
    }

    crate::serial_println!("[wayland] Surface {} → window {} created.", surf_id, wid);
}

/// Update the compositor window title when xdg_toplevel::set_title arrives.
pub fn set_title(client: &WaylandClient, surf_id: u32, title: &str) {
    if let Some(wid) = window_id_for_surface(client, surf_id) {
        crate::gui::compositor::set_window_title(wid, title);
    }
}

fn window_id_for_surface(client: &WaylandClient, surf_id: u32) -> Option<u64> {
    client.objects.get(&surf_id).and_then(|obj| {
        if let ObjectKind::Surface { window_id } = obj.kind {
            window_id
        } else {
            None
        }
    })
}

/// Find the title associated with an xdg_toplevel that references this surface.
fn find_toplevel_title(client: &WaylandClient, surf_id: u32) -> Option<String> {
    // Walk objects to find the xdg_surface that wraps surf_id, then find
    // the xdg_toplevel that wraps that xdg_surface.
    let xdg_surf_id = client.objects.values().find_map(|obj| {
        if let ObjectKind::XdgSurface { surface_id } = obj.kind {
            if surface_id == surf_id { Some(obj.id) } else { None }
        } else {
            None
        }
    })?;

    client.objects.values().find_map(|obj| {
        if let ObjectKind::XdgToplevel { title, surface_id } = &obj.kind {
            if *surface_id == xdg_surf_id && !title.is_empty() {
                Some(title.clone())
            } else {
                None
            }
        } else {
            None
        }
    })
}
