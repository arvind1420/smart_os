#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use smartsdk::gui::{Window, EVENT_MOUSE_CLICK, EVENT_KEY_PRESS};
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
//  Data Models
// ═══════════════════════════════════════════════════════════════

#[derive(Clone)]
struct MailItem {
    id: usize,
    from: String,
    to: String,
    subject: String,
    date: String,
    body: String,
    is_unread: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum AppView {
    Inbox,
    Compose,
}

#[derive(Clone, Copy, PartialEq)]
enum ActiveField {
    None,
    To,
    Subject,
    Body,
}

// ═══════════════════════════════════════════════════════════════
//  Color Palette
// ═══════════════════════════════════════════════════════════════

const BG_DARK: u32 = 0x0F0F13;
const BG_PANEL: u32 = 0x191921;
const NEON_CYAN: u32 = 0x00F3FF;
const NEON_PURPLE: u32 = 0xBD00FF;
const NEON_GREEN: u32 = 0x39FF14;
const NEON_RED: u32 = 0xFF3131;
const TEXT_MUTED: u32 = 0x888899;
const TEXT_WHITE: u32 = 0xFFFFFF;

// ═══════════════════════════════════════════════════════════════
//  VFS & EML Helpers
// ═══════════════════════════════════════════════════════════════

fn read_last_email_path() -> Option<String> {
    let path = "/tmp/last_email.txt";
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return None;
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
        core::str::from_utf8(slice).ok().map(|s| String::from(s))
    } else {
        None
    }
}

fn clear_last_email_path() {
    let path = "/tmp/last_email.txt";
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        2 // Write-only
    );
    if fd != u64::MAX {
        smartsdk::syscall::syscall3(
            smartsdk::syscall::SYS_WRITE,
            fd,
            b"".as_ptr() as u64,
            0
        );
        smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    }
}

fn read_file_content(path: &str) -> Option<String> {
    let fd = smartsdk::syscall::syscall3(
        smartsdk::syscall::SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0
    );
    if fd == u64::MAX {
        return None;
    }
    
    let mut content = String::new();
    let mut chunk = [0u8; 1024];
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
        if let Ok(s) = core::str::from_utf8(&chunk[..n as usize]) {
            content.push_str(s);
        } else {
            break;
        }
    }
    smartsdk::syscall::syscall1(smartsdk::syscall::SYS_CLOSE, fd);
    if content.is_empty() { None } else { Some(content) }
}

fn parse_eml(content: &str, id: usize) -> MailItem {
    let mut from = String::from("unknown@smartos.org");
    let mut to = String::from("user@smartos.org");
    let mut subject = String::from("(No Subject)");
    let mut date = String::from("Unknown Date");
    let mut body = String::new();

    let mut in_body = false;
    for line in content.lines() {
        if in_body {
            body.push_str(line);
            body.push('\n');
        } else {
            if line.is_empty() {
                in_body = true;
            } else if line.starts_with("From: ") {
                from = String::from(line[6..].trim());
            } else if line.starts_with("To: ") {
                to = String::from(line[4..].trim());
            } else if line.starts_with("Subject: ") {
                subject = String::from(line[9..].trim());
            } else if line.starts_with("Date: ") {
                date = String::from(line[5..].trim());
            }
        }
    }

    MailItem {
        id,
        from,
        to,
        subject,
        date,
        body,
        is_unread: false,
    }
}

// ═══════════════════════════════════════════════════════════════
//  UI Drawing Utilities
// ═══════════════════════════════════════════════════════════════

fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            wrapped.push(String::new());
            continue;
        }
        let mut words = paragraph.split(' ');
        let mut current_line = String::new();
        while let Some(word) = words.next() {
            if current_line.is_empty() {
                current_line.push_str(word);
            } else if current_line.len() + 1 + word.len() <= max_chars {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                wrapped.push(current_line);
                current_line = String::from(word);
            }
        }
        if !current_line.is_empty() {
            wrapped.push(current_line);
        }
    }
    wrapped
}

fn draw_btn(win: &Window, x: u16, y: u16, w: u16, h: u16, label: &str, border_color: u32, fill_color: u32) {
    win.fill_rect(x, y, w, h, border_color);
    win.fill_rect(x + 1, y + 1, w - 2, h - 2, fill_color);
    let text_x = x + (w - (label.len() * 8) as u16) / 2;
    let text_y = y + (h - 12) / 2;
    win.draw_text(text_x, text_y, label);
}

fn draw_input_box(win: &Window, label: &str, text: &str, x_lbl: u16, y_lbl: u16, x_box: u16, y_box: u16, w_box: u16, h_box: u16, is_active: bool) {
    win.draw_text(x_lbl, y_lbl, label);
    win.fill_rect(x_box, y_box, w_box, h_box, BG_DARK);
    let border_color = if is_active { NEON_GREEN } else { 0x444455 };
    win.fill_rect(x_box, y_box, w_box, 1, border_color);
    win.fill_rect(x_box, y_box + h_box - 1, w_box, 1, border_color);
    win.fill_rect(x_box, y_box, 1, h_box, border_color);
    win.fill_rect(x_box + w_box - 1, y_box, 1, h_box, border_color);
    
    let mut display_text = String::from(text);
    if is_active {
        display_text.push('_');
    }
    win.draw_text(x_box + 6, y_box + (h_box - 12) / 2, &display_text);
}

fn draw_body_input_box(win: &Window, text: &str, x_box: u16, y_box: u16, w_box: u16, h_box: u16, is_active: bool) {
    win.fill_rect(x_box, y_box, w_box, h_box, BG_DARK);
    let border_color = if is_active { NEON_GREEN } else { 0x444455 };
    win.fill_rect(x_box, y_box, w_box, 1, border_color);
    win.fill_rect(x_box, y_box + h_box - 1, w_box, 1, border_color);
    win.fill_rect(x_box, y_box, 1, h_box, border_color);
    win.fill_rect(x_box + w_box - 1, y_box, 1, h_box, border_color);
    
    let lines = wrap_text(text, 38);
    for (i, line) in lines.iter().enumerate() {
        let mut display_line = line.clone();
        if is_active && i == lines.len() - 1 {
            display_line.push('_');
        }
        if 10 + i * 16 + 12 < h_box as usize {
            win.draw_text(x_box + 6, y_box + 10 + (i * 16) as u16, &display_line);
        }
    }
    if is_active && lines.is_empty() {
        win.draw_text(x_box + 6, y_box + 10, "_");
    }
}

// ═══════════════════════════════════════════════════════════════
//  Main Entry
// ═══════════════════════════════════════════════════════════════

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    print("Starting Smart Email Client...\n");

    let mut inbox_list = alloc::vec![
        MailItem {
            id: 1,
            from: String::from("security@smartos.org"),
            to: String::from("user@smartos.org"),
            subject: String::from("Security Alert: Node green"),
            date: String::from("May 28, 2026"),
            body: String::from("We detected a new user-space environment initialization. All security systems are green.\n\nSmart OS Kernel Team\n"),
            is_unread: false,
        },
        MailItem {
            id: 2,
            from: String::from("copilot@smartos.org"),
            to: String::from("user@smartos.org"),
            subject: String::from("AI Copilot Suggestion"),
            date: String::from("May 27, 2026"),
            body: String::from("Based on your recent documents, I suggest creating a new backup of your projects folder.\n\nLocal NPU Model v2.5\n"),
            is_unread: true,
        },
        MailItem {
            id: 3,
            from: String::from("billing@smartos.org"),
            to: String::from("user@smartos.org"),
            subject: String::from("Invoice for Cloud Sync"),
            date: String::from("May 26, 2026"),
            body: String::from("Your invoice for this month's decentralized cloud sync is ready. Total due: 0.00 Credits.\n\nBilling Systems\n"),
            is_unread: false,
        }
    ];

    let mut selected_id: Option<usize> = Some(1);
    let mut view = AppView::Inbox;
    let mut active_field = ActiveField::None;

    // Compose fields
    let mut compose_to = String::new();
    let mut compose_subject = String::new();
    let mut compose_body = String::new();

    // Alert messages
    let mut notification_message: Option<String> = None;
    let mut notification_timer = 0usize;

    // Try reading last double-clicked email from File Manager
    if let Some(path) = read_last_email_path() {
        if !path.is_empty() {
            print("Detected external mail request from File Manager...\n");
            if let Some(content) = read_file_content(&path) {
                let parsed = parse_eml(&content, 0);
                inbox_list.insert(0, parsed);
                selected_id = Some(0);
            }
            clear_last_email_path();
        }
    }

    if let Some(win) = Window::new(600, 480) {
        // Register toolbar buttons in window manager
        let inbox_btn = win.add_button(10, 10, 70, 28);
        let compose_btn = win.add_button(90, 10, 90, 28);
        let refresh_btn = win.add_button(190, 10, 80, 28);
        let delete_btn = win.add_button(280, 10, 80, 28);

        let draw_screen = |win: &Window,
                           view: AppView,
                           active_fld: ActiveField,
                           sel_id: Option<usize>,
                           to_str: &str,
                           subj_str: &str,
                           body_str: &str,
                           notif: &Option<String>,
                           inbox: &[MailItem]| {
            // Draw window background
            win.fill_rect(0, 0, 600, 480, BG_DARK);

            // Draw header bar surface
            win.fill_rect(0, 0, 600, 48, BG_PANEL);

            // Draw toolbar buttons
            draw_btn(win, 10, 10, 70, 28, "Inbox", if view == AppView::Inbox { NEON_CYAN } else { 0x444455 }, BG_PANEL);
            draw_btn(win, 90, 10, 90, 28, "Compose", if view == AppView::Compose { NEON_CYAN } else { 0x444455 }, BG_PANEL);
            draw_btn(win, 190, 10, 80, 28, "Refresh", 0x444455, BG_PANEL);
            draw_btn(win, 280, 10, 80, 28, "Delete", 0xFF3131, BG_PANEL);

            // Status Bar
            win.draw_text(380, 18, "Status: SECURE NODE CONNECTED");

            // Left Pane: Inbox List
            win.draw_text(15, 60, "INBOX DIRECTORY");
            win.fill_rect(10, 80, 220, 390, BG_PANEL);
            // Borders
            win.fill_rect(10, 80, 220, 1, NEON_CYAN);
            win.fill_rect(10, 470, 220, 1, NEON_CYAN);
            win.fill_rect(10, 80, 1, 390, NEON_CYAN);
            win.fill_rect(230, 80, 1, 390, NEON_CYAN);

            // Render inbox entries
            for (i, mail) in inbox.iter().enumerate() {
                if i >= 5 { break; } // limit to 5 fits nicely
                let y_off = 85 + (i as u16 * 72);

                let is_selected = sel_id == Some(mail.id);
                if is_selected {
                    win.fill_rect(12, y_off, 216, 68, 0x1A2B35);
                }

                // Render content
                let from_line = format!("From: {}", mail.from);
                let mut from_disp = from_line;
                if from_disp.len() > 22 {
                    from_disp.truncate(20);
                    from_disp.push_str("..");
                }
                win.draw_text(16, y_off + 6, &from_disp);

                let subj_line = format!("Subj: {}", mail.subject);
                let mut subj_disp = subj_line;
                if subj_disp.len() > 22 {
                    subj_disp.truncate(20);
                    subj_disp.push_str("..");
                }
                win.draw_text(16, y_off + 24, &subj_disp);

                win.draw_text(16, y_off + 44, &mail.date);

                // Unread dot indicator
                if mail.is_unread {
                    win.fill_rect(212, y_off + 8, 8, 8, NEON_GREEN);
                }

                // Divider line
                win.fill_rect(12, y_off + 70, 216, 1, 0x2E2E3A);
            }

            // Right Pane: Reader / Composer
            if view == AppView::Inbox {
                win.draw_text(250, 60, "DECRYPTED MAIL READER");
                win.fill_rect(240, 80, 350, 390, BG_PANEL);
                // Borders
                win.fill_rect(240, 80, 350, 1, NEON_PURPLE);
                win.fill_rect(240, 470, 350, 1, NEON_PURPLE);
                win.fill_rect(240, 80, 1, 390, NEON_PURPLE);
                win.fill_rect(590, 80, 1, 390, NEON_PURPLE);

                // Find selected mail
                let found_mail = sel_id.and_then(|id| inbox.iter().find(|m| m.id == id));
                if let Some(mail) = found_mail {
                    win.draw_text(250, 95, &format!("FROM: {}", mail.from));
                    win.draw_text(250, 115, &format!("TO  : {}", mail.to));
                    win.draw_text(250, 135, &format!("SUBJ: {}", mail.subject));
                    win.draw_text(250, 155, &format!("DATE: {}", mail.date));

                    win.fill_rect(250, 175, 330, 1, NEON_PURPLE);

                    // Body
                    let wrapped = wrap_text(&mail.body, 40);
                    for (k, line) in wrapped.iter().enumerate() {
                        if k >= 16 { break; }
                        win.draw_text(250, 190 + (k as u16 * 16), line);
                    }
                } else {
                    win.draw_text(340, 240, "No mail selected");
                }
            } else {
                // Compose view
                win.draw_text(250, 60, "OUTBOX MAIL COMPOSER");
                win.fill_rect(240, 80, 350, 390, BG_PANEL);
                // Borders
                win.fill_rect(240, 80, 350, 1, NEON_PURPLE);
                win.fill_rect(240, 470, 350, 1, NEON_PURPLE);
                win.fill_rect(240, 80, 1, 390, NEON_PURPLE);
                win.fill_rect(590, 80, 1, 390, NEON_PURPLE);

                // Input box fields
                draw_input_box(win, "TO:", to_str, 250, 96, 290, 90, 290, 24, active_fld == ActiveField::To);
                draw_input_box(win, "SUBJ:", subj_str, 250, 136, 300, 130, 280, 24, active_fld == ActiveField::Subject);

                win.draw_text(250, 175, "BODY:");
                draw_body_input_box(win, body_str, 250, 195, 330, 180, active_fld == ActiveField::Body);

                // Bottom control actions
                draw_btn(win, 250, 390, 110, 28, "Send Email", NEON_GREEN, BG_PANEL);
                draw_btn(win, 375, 390, 80, 28, "Cancel", NEON_RED, BG_PANEL);
            }

            // Notification Overlay Modal Dialog
            if let Some(msg) = notif {
                win.fill_rect(150, 180, 300, 120, 0x111116);
                let modal_border = NEON_GREEN;
                win.fill_rect(150, 180, 300, 1, modal_border);
                win.fill_rect(150, 299, 300, 1, modal_border);
                win.fill_rect(150, 180, 1, 120, modal_border);
                win.fill_rect(449, 180, 1, 120, modal_border);
                
                win.draw_text(170, 200, "NETWORK TRANSMITTER");
                win.draw_text(170, 216, "────────────────────────");
                win.draw_text(170, 242, msg);
                win.draw_text(170, 272, "[Click anywhere to dismiss]");
            }
        };

        draw_screen(&win, view, active_field, selected_id, &compose_to, &compose_subject, &compose_body, &notification_message, &inbox_list);

        loop {
            while let Some(ev) = Window::poll_event() {
                if ev.event_type == EVENT_MOUSE_CLICK {
                    let clicked_id = ev.data[2] as u8;
                    let mx = ev.data[0] as u16;
                    let my = ev.data[1] as u16;

                    // If notification is active, any click clears it
                    if notification_message.is_some() {
                        notification_message = None;
                        notification_timer = 0;
                        draw_screen(&win, view, active_field, selected_id, &compose_to, &compose_subject, &compose_body, &notification_message, &inbox_list);
                        continue;
                    }

                    if clicked_id == inbox_btn {
                        view = AppView::Inbox;
                        active_field = ActiveField::None;
                    } else if clicked_id == compose_btn {
                        view = AppView::Compose;
                        active_field = ActiveField::To;
                    } else if clicked_id == refresh_btn {
                        // Dynamically append new incoming message from server
                        let admin_mail_id = 999;
                        if !inbox_list.iter().any(|m| m.id == admin_mail_id) {
                            let admin_mail = MailItem {
                                id: admin_mail_id,
                                from: String::from("admin@smartos.org"),
                                to: String::from("user@smartos.org"),
                                subject: String::from("System Status Report - Green"),
                                date: String::from("Just Now"),
                                body: String::from("All microkernels and user-space subsystems are communicating perfectly.\nCPU load: 1.8%.\nGPU-acceleration: Active.\nP2P sync status: Connected.\n"),
                                is_unread: true,
                            };
                            inbox_list.insert(0, admin_mail);
                            selected_id = Some(admin_mail_id);
                            notification_message = Some(String::from("New mail synchronized!"));
                            notification_timer = 90;
                        } else {
                            notification_message = Some(String::from("Mailbox is up to date."));
                            notification_timer = 60;
                        }
                    } else if clicked_id == delete_btn {
                        if let Some(sel_id) = selected_id {
                            if let Some(pos) = inbox_list.iter().position(|m| m.id == sel_id) {
                                inbox_list.remove(pos);
                                notification_message = Some(String::from("Message deleted."));
                                notification_timer = 60;
                            }
                            selected_id = inbox_list.first().map(|m| m.id);
                        } else {
                            notification_message = Some(String::from("No message selected."));
                            notification_timer = 60;
                        }
                    } else {
                        // Click hit testing in body view area
                        if view == AppView::Compose {
                            // Check Send/Cancel buttons
                            if mx >= 250 && mx <= 360 && my >= 390 && my <= 418 {
                                // Send Email
                                if compose_to.is_empty() {
                                    notification_message = Some(String::from("Error: Recipient (TO) required"));
                                    notification_timer = 60;
                                } else {
                                    let new_id = 1000 + inbox_list.len();
                                    let new_mail = MailItem {
                                        id: new_id,
                                        from: String::from("user@smartos.org"),
                                        to: compose_to.clone(),
                                        subject: if compose_subject.is_empty() { String::from("(No Subject)") } else { compose_subject.clone() },
                                        date: String::from("Just Now"),
                                        body: compose_body.clone(),
                                        is_unread: false,
                                    };
                                    inbox_list.insert(0, new_mail);
                                    selected_id = Some(new_id);

                                    notification_message = Some(String::from("Email delivered successfully."));
                                    notification_timer = 90;

                                    // Clear fields
                                    compose_to.clear();
                                    compose_subject.clear();
                                    compose_body.clear();
                                    active_field = ActiveField::None;
                                    view = AppView::Inbox;
                                }
                            } else if mx >= 375 && mx <= 455 && my >= 390 && my <= 418 {
                                // Cancel
                                compose_to.clear();
                                compose_subject.clear();
                                compose_body.clear();
                                active_field = ActiveField::None;
                                view = AppView::Inbox;
                                notification_message = Some(String::from("Composition discarded."));
                                notification_timer = 60;
                            }
                            // Check Text fields focus
                            else if mx >= 290 && mx <= 580 && my >= 90 && my <= 114 {
                                active_field = ActiveField::To;
                            } else if mx >= 300 && mx <= 580 && my >= 130 && my <= 154 {
                                active_field = ActiveField::Subject;
                            } else if mx >= 250 && mx <= 580 && my >= 195 && my <= 375 {
                                active_field = ActiveField::Body;
                            } else {
                                active_field = ActiveField::None;
                            }
                        }

                        // Left panel list items hit testing (both views)
                        if mx >= 10 && mx <= 230 && my >= 80 && my <= 470 {
                            let item_idx = ((my - 80) / 72) as usize;
                            if item_idx < inbox_list.len() {
                                inbox_list[item_idx].is_unread = false;
                                selected_id = Some(inbox_list[item_idx].id);
                                view = AppView::Inbox;
                            }
                        }
                    }
                } else if ev.event_type == EVENT_KEY_PRESS {
                    let char_code = ev.data[0] as u8;

                    if view == AppView::Compose && active_field != ActiveField::None {
                        let target_str = match active_field {
                            ActiveField::To => &mut compose_to,
                            ActiveField::Subject => &mut compose_subject,
                            ActiveField::Body => &mut compose_body,
                            ActiveField::None => unreachable!(),
                        };

                        if char_code == 0x08 { // Backspace
                            target_str.pop();
                        } else if char_code == 0x0D || char_code == b'\n' {
                            if active_field == ActiveField::Body {
                                target_str.push('\n');
                            } else if active_field == ActiveField::To {
                                active_field = ActiveField::Subject;
                            } else if active_field == ActiveField::Subject {
                                active_field = ActiveField::Body;
                            }
                        } else if char_code >= 0x20 && char_code <= 0x7E {
                            target_str.push(char_code as char);
                        }
                    }
                }
            }

            if notification_timer > 0 {
                notification_timer -= 1;
                if notification_timer == 0 {
                    notification_message = None;
                }
            }

            draw_screen(&win, view, active_field, selected_id, &compose_to, &compose_subject, &compose_body, &notification_message, &inbox_list);

            // Yield / spin for locking frame rate
            for _ in 0..1_500_000 { core::hint::spin_loop(); }
        }
    }

    smartsdk::syscall::exit(0);
}
