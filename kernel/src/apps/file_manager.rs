/// Smart OS File Manager — Visual file browser application.
///
/// Provides directory browsing with file type icons (from AI classification),
/// file preview, and navigation via clicks.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

/// A directory entry with metadata.
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub icon: &'static str,
    pub icon_color: Color,
}

/// File manager application state.
pub struct FileManagerState {
    pub window_id: WindowId,
    pub current_path: String,
    pub entries: Vec<DirEntry>,
    pub preview_lines: Vec<(String, Color)>,
    pub dirty: bool,
}

pub static STATE: Mutex<Option<FileManagerState>> = Mutex::new(None);

/// File manager thread entry point.
pub fn run() {
    // Create the file manager window
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let mut win = Window::new("Files", 520, 35, 440, 340, ACCENT_ORANGE);
        win.use_widgets = true;

        // Widget 0: Path bar (label showing current directory)
        let path_label = Widget::new(0, 4, 2, 432, 18,
            WidgetKind::Label(StaticLabel::new("/", ACCENT_ORANGE)));

        // Widget 1: Directory listing (left side)
        let dir_list = Widget::new(1, 0, 22, 220, 314,
            WidgetKind::ScrollText(ScrollableText::new(200)));

        // Widget 2: File preview (right side)
        let preview = Widget::new(2, 224, 22, 216, 314,
            WidgetKind::ScrollText(ScrollableText::new(200)));

        win.add_widget(path_label);
        win.add_widget(dir_list);
        win.add_widget(preview);

        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Initialize state and load root directory
    let entries = load_directory("/");
    *STATE.lock() = Some(FileManagerState {
        window_id,
        current_path: String::from("/"),
        entries,
        preview_lines: vec![(String::from("  Select a file to preview"), TEXT_MUTED)],
        dirty: true,
    });

    // Main loop
    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            match action {
                WidgetAction::Execute(AppCommand::LineClicked(line_idx)) => {
                    let mut state = STATE.lock();
                    if let Some(ref mut s) = *state {
                        handle_line_click(s, line_idx);
                    }
                }
                _ => {}
            }
        }

        for _ in 0..5 {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Handle a click on a directory listing line.
fn handle_line_click(state: &mut FileManagerState, line_idx: usize) {
    // Line 0 is "[..] Back", lines 1+ are entries
    if line_idx == 0 {
        // Go back to parent
        let parent = if state.current_path == "/" {
            String::from("/")
        } else {
            match state.current_path.rfind('/') {
                Some(0) => String::from("/"),
                Some(pos) => String::from(&state.current_path[..pos]),
                None => String::from("/"),
            }
        };
        state.current_path = parent.clone();
        state.entries = load_directory(&parent);
        state.preview_lines = vec![(String::from("  Select a file to preview"), TEXT_MUTED)];
        state.dirty = true;
    } else {
        let entry_idx = line_idx - 1; // -1 for the "[..] Back" line
        if entry_idx < state.entries.len() {
            let entry_name = state.entries[entry_idx].name.clone();
            let is_dir = state.entries[entry_idx].is_dir;

            let full_path = if state.current_path == "/" {
                format!("/{}", entry_name)
            } else {
                format!("{}/{}", state.current_path, entry_name)
            };

            if is_dir {
                // Navigate into directory
                state.current_path = full_path.clone();
                state.entries = load_directory(&full_path);
                state.preview_lines = vec![(String::from("  Select a file to preview"), TEXT_MUTED)];
            } else {
                // Check if it is an image file
                let lower = entry_name.to_ascii_lowercase();
                if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".bmp") || lower.ends_with(".webp") || lower.ends_with(".heic") || lower.ends_with(".raw") {
                    // Write full path to /tmp/last_image.txt
                    let _ = crate::vfs::create_and_write("/tmp/last_image.txt", full_path.as_bytes());
                    // Spawn user-space image viewer
                    let _ = crate::process::scheduler::spawn_user_process("image_viewer", "/bin/image_viewer");
                } else if lower.ends_with(".pdf") {
                    // Write full path to /tmp/last_pdf.txt
                    let _ = crate::vfs::create_and_write("/tmp/last_pdf.txt", full_path.as_bytes());
                    // Spawn user-space PDF reader
                    let _ = crate::process::scheduler::spawn_user_process("pdf_reader", "/bin/pdf_reader");
                } else if lower.ends_with(".mp4") || lower.ends_with(".avi") || lower.ends_with(".mkv") || lower.ends_with(".mov") {
                    // Write full path to /tmp/last_video.txt
                    let _ = crate::vfs::create_and_write("/tmp/last_video.txt", full_path.as_bytes());
                    // Spawn user-space video player
                    let _ = crate::process::scheduler::spawn_user_process("video_player", "/bin/video_player");
                } else if lower.ends_with(".eml") {
                    // Write full path to /tmp/last_email.txt
                    let _ = crate::vfs::create_and_write("/tmp/last_email.txt", full_path.as_bytes());
                    // Spawn user-space email client
                    let _ = crate::process::scheduler::spawn_user_process("email_client", "/bin/email_client");
                } else if lower.ends_with(".docx") {
                    // Write full path to /tmp/last_doc.txt
                    let _ = crate::vfs::create_and_write("/tmp/last_doc.txt", full_path.as_bytes());
                    // Spawn user-space office app
                    let _ = crate::process::scheduler::spawn_user_process("office", "/bin/office");
                }
                // Preview the file
                state.preview_lines = load_file_preview(&full_path);
            }
            state.dirty = true;
        }
    }
}

/// Load directory entries from VFS.
fn load_directory(path: &str) -> Vec<DirEntry> {
    let mut entries = Vec::new();

    if let Ok(names) = crate::vfs::readdir(path) {
        for name in names {
            let full_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path, name)
            };

            let is_dir = crate::vfs::readdir(&full_path).is_ok();

            let size = if !is_dir {
                crate::vfs::stat(&full_path).ok()
                    .and_then(|v| v.as_map().and_then(|m|
                        m.iter().find(|(k, _)| k.as_str() == Some("size"))
                            .and_then(|(_, v)| v.as_u64())))
                    .unwrap_or(0)
            } else {
                0
            };

            let (icon, icon_color) = if is_dir {
                ("[D]", ACCENT_CYAN)
            } else {
                file_icon(&name)
            };

            entries.push(DirEntry { name, is_dir, size, icon, icon_color });
        }
    }

    // Sort: directories first, then files
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => core::cmp::Ordering::Less,
            (false, true) => core::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    entries
}

/// Get file icon based on extension.
fn file_icon(name: &str) -> (&'static str, Color) {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" | "c" | "h" | "py" | "js" => ("[S]", ACCENT_MAGENTA),
        "txt" | "md" | "log" => ("[T]", ACCENT_GREEN),
        "json" | "toml" | "yaml" | "yml" | "cfg" | "sp" => ("[C]", ACCENT_ORANGE),
        "png" | "jpg" | "bmp" | "gif" => ("[I]", ACCENT_PURPLE),
        "eml" => ("[E]", ACCENT_ORANGE),
        "docx" => ("[W]", ACCENT_CYAN),
        "zip" | "tar" | "gz" => ("[A]", ACCENT_BLUE),
        "bin" | "elf" | "exe" => ("[B]", ACCENT_RED),
        _ => ("[?]", TEXT_MUTED),
    }
}

/// Load first few lines of a file for preview.
fn load_file_preview(path: &str) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((format!("  {}", path), ACCENT_ORANGE));
    lines.push((String::from("  ────────────────────"), TEXT_MUTED));

    match crate::vfs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 2048];
            match crate::vfs::read(fd, &mut buf) {
                Ok(n) => {
                    crate::vfs::close(fd).ok();
                    lines.push((format!("  Size: {} bytes", n), TEXT_SECONDARY));
                    lines.push((String::new(), TEXT_PRIMARY));

                    let text = core::str::from_utf8(&buf[..n]).unwrap_or("<binary>");
                    let mut line_count = 0;
                    for line in text.lines() {
                        if line_count >= 15 { // Max preview lines
                            lines.push((String::from("  ..."), TEXT_MUTED));
                            break;
                        }
                        lines.push((format!("  {}", line), TEXT_PRIMARY));
                        line_count += 1;
                    }
                }
                Err(e) => {
                    crate::vfs::close(fd).ok();
                    lines.push((format!("  Error: {}", e), ACCENT_RED));
                }
            }
        }
        Err(e) => {
            lines.push((format!("  Error: {}", e), ACCENT_RED));
        }
    }

    lines
}

/// Sync file manager state to window widgets.
pub fn sync_to_window(window: &mut Window) {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };
    if !s.dirty { return; }
    s.dirty = false;

    // Update path label (widget 0)
    if let Some(widget) = window.get_widget_mut(0) {
        if let WidgetKind::Label(ref mut label) = widget.kind {
            label.text = format!("  {} {}", "\u{25C0}", s.current_path); // ◀ path
        }
    }

    // Update directory listing (widget 1)
    if let Some(widget) = window.get_widget_mut(1) {
        if let WidgetKind::ScrollText(ref mut scroll) = widget.kind {
            let mut lines = Vec::new();
            lines.push((String::from("  [..] Back"), TEXT_SECONDARY));
            for entry in &s.entries {
                let suffix = if entry.is_dir {
                    String::from("/")
                } else if entry.size >= 1024 {
                    format!("  {}K", entry.size / 1024)
                } else {
                    format!("  {}B", entry.size)
                };
                lines.push((format!("  {} {}{}", entry.icon, entry.name, suffix), entry.icon_color));
            }
            scroll.lines = lines;
            scroll.scroll_offset = 0;
        }
    }

    // Update preview (widget 2)
    if let Some(widget) = window.get_widget_mut(2) {
        if let WidgetKind::ScrollText(ref mut scroll) = widget.kind {
            scroll.lines = s.preview_lines.clone();
            scroll.scroll_offset = 0;
        }
    }
}
