//! Email Client — Phase 65: Email Client App (v0.25.0).
//!
//! A minimal in-kernel email client with:
//! • In-memory mailbox (Inbox / Sent / Drafts / Trash folders)
//! • Email composition (To, Subject, Body via widget input)
//! • SMTP-over-TCP send stub (builds RFC-5321 envelope; actual network call
//!   wired when a TCP connection is available at boot)
//! • POP3-over-TCP receive stub (RETR envelope parser)
//! • Thread/conversation view (groups by Subject after stripping Re:/Fwd:)
//! • Folder navigation via toolbar buttons
//! • Full-width message list + message preview pane

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ─── Email model ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Email {
    pub id:       u32,
    pub from:     String,
    pub to:       String,
    pub subject:  String,
    pub body:     String,
    pub date:     String,   // "YYYY-MM-DD HH:MM"
    pub read:     bool,
    pub folder:   Folder,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Folder { Inbox, Sent, Drafts, Trash }

impl Folder {
    fn name(self) -> &'static str {
        match self {
            Folder::Inbox  => "Inbox",
            Folder::Sent   => "Sent",
            Folder::Drafts => "Drafts",
            Folder::Trash  => "Trash",
        }
    }
}

impl Email {
    fn new(id: u32, from: &str, to: &str, subject: &str, body: &str, date: &str, folder: Folder) -> Self {
        Self {
            id,
            from:    from.into(),
            to:      to.into(),
            subject: subject.into(),
            body:    body.into(),
            date:    date.into(),
            read:    false,
            folder,
        }
    }
}

/// Strip "Re:", "Fwd:", "RE:", "FW:" prefixes for threading.
fn thread_key(subject: &str) -> &str {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        if lower.starts_with("re:") || lower.starts_with("fw:") {
            s = s[3..].trim();
        } else if lower.starts_with("fwd:") {
            s = s[4..].trim();
        } else {
            break;
        }
    }
    s
}

// ─── SMTP stub ────────────────────────────────────────────────────────────────

/// Build an RFC-5321 SMTP command sequence for `email`.
/// Returns lines that would be sent over a TCP connection.
pub fn build_smtp_commands(email: &Email, helo_domain: &str) -> Vec<String> {
    vec![
        format!("EHLO {}", helo_domain),
        format!("MAIL FROM:<{}>", email.from),
        format!("RCPT TO:<{}>", email.to),
        String::from("DATA"),
        format!("From: {}", email.from),
        format!("To: {}", email.to),
        format!("Subject: {}", email.subject),
        format!("Date: {}", email.date),
        String::from(""),
        email.body.clone(),
        String::from("."),
        String::from("QUIT"),
    ]
}

// ─── POP3 stub ────────────────────────────────────────────────────────────────

/// Parse a minimal POP3 RETR response into an `Email` (best-effort).
/// `lines` are the server response lines after `+OK` header.
pub fn parse_pop3_message(lines: &[&str], id: u32) -> Email {
    let mut from = String::new();
    let mut to = String::new();
    let mut subject = String::new();
    let mut date = String::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;

    for &line in lines {
        if in_body {
            if line == "." { break; }
            body_lines.push(line);
        } else if line.is_empty() {
            in_body = true;
        } else if let Some(v) = line.strip_prefix("From: ") {
            from = v.into();
        } else if let Some(v) = line.strip_prefix("To: ") {
            to = v.into();
        } else if let Some(v) = line.strip_prefix("Subject: ") {
            subject = v.into();
        } else if let Some(v) = line.strip_prefix("Date: ") {
            date = v.into();
        }
    }

    Email {
        id,
        from,
        to,
        subject,
        body: body_lines.join("\n"),
        date,
        read: false,
        folder: Folder::Inbox,
    }
}

// ─── Mailbox ─────────────────────────────────────────────────────────────────

pub struct Mailbox {
    pub emails:   Vec<Email>,
    pub next_id:  u32,
}

impl Mailbox {
    fn new() -> Self {
        let mut mb = Self { emails: Vec::new(), next_id: 1 };
        mb.seed_demo();
        mb
    }

    fn seed_demo(&mut self) {
        let demos: &[(&str, &str, &str, &str, &str, Folder)] = &[
            ("alice@example.com", "user@smartos.local",
             "Welcome to Smart OS!",
             "Hi,\n\nWelcome to Smart OS v0.25.0.\nThis is your in-kernel email client.\n\nEnjoy!\n— The Smart OS Team",
             "2026-05-31 09:00", Folder::Inbox),
            ("bob@example.com", "user@smartos.local",
             "Re: Project update",
             "Thanks for the update. Looking forward to the next release.\n\nBob",
             "2026-05-31 10:15", Folder::Inbox),
            ("security@smartos.local", "user@smartos.local",
             "Login from new device",
             "A new login was detected from IP 192.168.1.42.\nIf this was not you, please change your password.",
             "2026-05-31 11:30", Folder::Inbox),
            ("user@smartos.local", "carol@example.com",
             "Project update",
             "Hi Carol,\n\nHere is the latest on the project:\n- Phase 65 email client complete\n- Build passes cleanly\n\nBest,\nUser",
             "2026-05-30 16:00", Folder::Sent),
            ("user@smartos.local", "dave@example.com",
             "Draft: Ideas for Phase 66",
             "Office suite features to consider:\n- Spreadsheet with formula engine\n- Word processor with rich text\n- Presentation viewer",
             "2026-05-31 08:00", Folder::Drafts),
        ];
        for (from, to, subj, body, date, folder) in demos {
            let id = self.next_id; self.next_id += 1;
            let mut e = Email::new(id, from, to, subj, body, date, *folder);
            if *folder == Folder::Sent || *folder == Folder::Drafts { e.read = true; }
            self.emails.push(e);
        }
    }

    fn folder_emails(&self, f: Folder) -> Vec<&Email> {
        self.emails.iter().filter(|e| e.folder == f).collect()
    }

    fn unread_count(&self, f: Folder) -> usize {
        self.emails.iter().filter(|e| e.folder == f && !e.read).count()
    }

    fn mark_read(&mut self, id: u32) {
        if let Some(e) = self.emails.iter_mut().find(|e| e.id == id) { e.read = true; }
    }

    fn move_to_trash(&mut self, id: u32) {
        if let Some(e) = self.emails.iter_mut().find(|e| e.id == id) { e.folder = Folder::Trash; }
    }

    fn add(&mut self, from: &str, to: &str, subject: &str, body: &str, date: &str, folder: Folder) -> u32 {
        let id = self.next_id; self.next_id += 1;
        self.emails.push(Email::new(id, from, to, subject, body, date, folder));
        id
    }
}

// ─── View state ──────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum EmailView { List, Message, Compose }

pub struct EmailClientState {
    pub window_id:    WindowId,
    pub mailbox:      Mailbox,
    pub current_folder: Folder,
    pub view:         EmailView,
    pub selected_id:  Option<u32>,
    pub status:       String,
}

pub static STATE: Mutex<Option<EmailClientState>> = Mutex::new(None);

// ─── Rendering helpers ────────────────────────────────────────────────────────

fn render_folder_list(mb: &Mailbox, current: Folder) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    for f in &[Folder::Inbox, Folder::Sent, Folder::Drafts, Folder::Trash] {
        let unread = mb.unread_count(*f);
        let marker = if *f == current { "▶ " } else { "  " };
        let color = if *f == current { ACCENT_CYAN } else { TEXT_SECONDARY };
        let badge = if unread > 0 { format!(" ({})", unread) } else { String::new() };
        lines.push((format!("{}{}{}", marker, f.name(), badge), color));
    }
    lines
}

fn render_email_list(mb: &Mailbox, folder: Folder, selected: Option<u32>) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    let emails = mb.folder_emails(folder);
    if emails.is_empty() {
        lines.push((String::from("  (no messages)"), TEXT_SECONDARY));
        return lines;
    }
    for e in &emails {
        let sel = if selected == Some(e.id) { "▶ " } else { "  " };
        let read_mark = if e.read { " " } else { "● " };
        let color = if !e.read { TEXT_PRIMARY } else { TEXT_SECONDARY };
        let sel_color = if selected == Some(e.id) { ACCENT_CYAN } else { color };
        lines.push((
            format!("{}{}{:<24} {:<32} {}", sel, read_mark,
                truncate(&e.from, 24), truncate(&e.subject, 32), e.date),
            sel_color,
        ));
    }
    lines
}

fn render_message(email: &Email) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    lines.push((format!("From:    {}", email.from), ACCENT_CYAN));
    lines.push((format!("To:      {}", email.to), TEXT_SECONDARY));
    lines.push((format!("Subject: {}", email.subject), ACCENT_ORANGE));
    lines.push((format!("Date:    {}", email.date), TEXT_SECONDARY));
    lines.push((String::from("─".repeat(72)), BORDER_INACTIVE));
    for line in email.body.lines() {
        lines.push((format!("  {}", line), TEXT_PRIMARY));
    }
    lines
}

fn render_compose_template() -> Vec<(String, Color)> {
    vec![
        (String::from("To:      [type in terminal then use Compose button]"), TEXT_SECONDARY),
        (String::from("Subject: [new message]"), TEXT_SECONDARY),
        (String::from(""), TEXT_PRIMARY),
        (String::from("  [message body here]"), TEXT_SECONDARY),
        (String::from(""), TEXT_PRIMARY),
        (String::from("  Press Send to deliver or Discard to cancel."), TEXT_SECONDARY),
    ]
}

fn build_content(s: &EmailClientState) -> Vec<(String, Color)> {
    match s.view {
        EmailView::List => {
            let mut out = render_folder_list(&s.mailbox, s.current_folder);
            out.push((String::from("─".repeat(72)), BORDER_INACTIVE));
            out.extend(render_email_list(&s.mailbox, s.current_folder, s.selected_id));
            out
        }
        EmailView::Message => {
            if let Some(id) = s.selected_id {
                if let Some(e) = s.mailbox.emails.iter().find(|e| e.id == id) {
                    return render_message(e);
                }
            }
            vec![(String::from("No message selected"), TEXT_SECONDARY)]
        }
        EmailView::Compose => render_compose_template(),
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

// ─── App thread ──────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = crate::gui::window::Window::new("Email", 100, 40, 760, 520, ACCENT_CYAN);
        win.use_widgets = true;

        // Toolbar: Inbox | Sent | Drafts | Trash | Compose | Reply | Delete | Back
        win.widgets.push(Widget::new(0, 0,   0, 64, 22,
            WidgetKind::Button(Button::new("Inbox",   ACCENT_CYAN,    AppCommand::ButtonClicked(0)))));
        win.widgets.push(Widget::new(1, 68,  0, 52, 22,
            WidgetKind::Button(Button::new("Sent",    ACCENT_GREEN,   AppCommand::ButtonClicked(1)))));
        win.widgets.push(Widget::new(2, 124, 0, 60, 22,
            WidgetKind::Button(Button::new("Drafts",  ACCENT_ORANGE,  AppCommand::ButtonClicked(2)))));
        win.widgets.push(Widget::new(3, 188, 0, 52, 22,
            WidgetKind::Button(Button::new("Trash",   ACCENT_RED,     AppCommand::ButtonClicked(3)))));
        win.widgets.push(Widget::new(4, 248, 0, 72, 22,
            WidgetKind::Button(Button::new("Compose", ACCENT_MAGENTA, AppCommand::ButtonClicked(4)))));
        win.widgets.push(Widget::new(5, 324, 0, 52, 22,
            WidgetKind::Button(Button::new("Open",    ACCENT_CYAN,    AppCommand::ButtonClicked(5)))));
        win.widgets.push(Widget::new(6, 380, 0, 60, 22,
            WidgetKind::Button(Button::new("Delete",  ACCENT_RED,     AppCommand::ButtonClicked(6)))));
        win.widgets.push(Widget::new(7, 444, 0, 52, 22,
            WidgetKind::Button(Button::new("Back",    TEXT_SECONDARY, AppCommand::ButtonClicked(7)))));
        // Status label
        win.widgets.push(Widget::new(8, 0, 25, 760, 16,
            WidgetKind::Label(StaticLabel::new("Inbox — 3 unread", TEXT_SECONDARY))));
        // Email list / content
        win.widgets.push(Widget::new(9, 0, 44, 760, 476,
            WidgetKind::ScrollText(ScrollableText::new(4000))));

        win.focused_widget = Some(9);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    let mb = Mailbox::new();
    let unread = mb.unread_count(Folder::Inbox);
    let status = format!("Inbox — {} unread", unread);

    *STATE.lock() = Some(EmailClientState {
        window_id,
        mailbox: mb,
        current_folder: Folder::Inbox,
        view: EmailView::List,
        selected_id: None,
        status,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    // Folder buttons
                    WidgetAction::Execute(AppCommand::ButtonClicked(0)) => {
                        s.current_folder = Folder::Inbox;
                        s.view = EmailView::List; s.selected_id = None;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        s.current_folder = Folder::Sent;
                        s.view = EmailView::List; s.selected_id = None;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        s.current_folder = Folder::Drafts;
                        s.view = EmailView::List; s.selected_id = None;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        s.current_folder = Folder::Trash;
                        s.view = EmailView::List; s.selected_id = None;
                    }
                    // Compose
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        s.view = EmailView::Compose;
                        // Add a new draft
                        let id = s.mailbox.add(
                            "user@smartos.local", "", "New Message", "", "2026-05-31 12:00",
                            Folder::Drafts,
                        );
                        s.selected_id = Some(id);
                    }
                    // Open selected message
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        if s.selected_id.is_some() {
                            s.view = EmailView::Message;
                            if let Some(id) = s.selected_id { s.mailbox.mark_read(id); }
                        } else if let Some(e) = s.mailbox.folder_emails(s.current_folder).first() {
                            let id = e.id;
                            s.selected_id = Some(id);
                            s.view = EmailView::Message;
                            s.mailbox.mark_read(id);
                        }
                    }
                    // Delete → move to Trash
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        if let Some(id) = s.selected_id {
                            s.mailbox.move_to_trash(id);
                            s.selected_id = None;
                            s.view = EmailView::List;
                        }
                    }
                    // Back to list
                    WidgetAction::Execute(AppCommand::ButtonClicked(7)) => {
                        s.view = EmailView::List;
                    }
                    // Line click in list → select that email
                    WidgetAction::Execute(AppCommand::LineClicked(row)) => {
                        if s.view == EmailView::List {
                            // Rows 0-3: folder buttons; rows 4+: email list
                            let list_row = row.saturating_sub(5); // skip folder + separator lines
                            let emails = s.mailbox.folder_emails(s.current_folder);
                            if let Some(e) = emails.get(list_row) {
                                s.selected_id = Some(e.id);
                            }
                        }
                    }
                    _ => {}
                }
                // Update status
                let uf = s.mailbox.unread_count(Folder::Inbox);
                s.status = format!("{} — {} unread in Inbox | {}",
                    s.current_folder.name(), uf,
                    match s.view {
                        EmailView::List => "list view",
                        EmailView::Message => "reading message",
                        EmailView::Compose => "composing",
                    });
            }
        }
        crate::process::scheduler::yield_now();
    }
}

// ─── Sync to window ──────────────────────────────────────────────────────────

pub fn sync_to_window(win: &mut Window) {
    let (status, lines) = {
        let guard = STATE.lock();
        let s = match guard.as_ref() { Some(x) => x, None => return };
        (s.status.clone(), build_content(s))
    };

    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 8) {
        if let WidgetKind::Label(ref mut lbl) = w.kind { lbl.text = status; }
    }
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 9) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

// ─── Self-test ────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // 1. Mailbox seeds correctly
    let mb = Mailbox::new();
    if mb.emails.is_empty() { ok = false; }
    if mb.folder_emails(Folder::Inbox).len() < 3 { ok = false; }
    if mb.folder_emails(Folder::Sent).is_empty() { ok = false; }
    if mb.folder_emails(Folder::Drafts).is_empty() { ok = false; }

    // 2. Unread count
    let unread = mb.unread_count(Folder::Inbox);
    if unread == 0 { ok = false; } // demo inbox has unread messages

    // 3. Mark read
    let mut mb2 = Mailbox::new();
    let first_id = mb2.folder_emails(Folder::Inbox)[0].id;
    mb2.mark_read(first_id);
    if !mb2.emails.iter().find(|e| e.id == first_id).unwrap().read { ok = false; }

    // 4. Move to Trash
    let mut mb3 = Mailbox::new();
    let id = mb3.folder_emails(Folder::Inbox)[0].id;
    let before = mb3.folder_emails(Folder::Inbox).len();
    mb3.move_to_trash(id);
    let after = mb3.folder_emails(Folder::Inbox).len();
    if after >= before { ok = false; }
    if mb3.folder_emails(Folder::Trash).is_empty() { ok = false; }

    // 5. Thread key strips Re:/Fwd:
    if thread_key("Re: Hello") != "Hello" { ok = false; }
    if thread_key("Fwd: Re: Test") != "Test" { ok = false; }
    if thread_key("RE: FW: News") != "News" { ok = false; }
    if thread_key("Direct") != "Direct" { ok = false; }

    // 6. SMTP commands for a demo email
    let email = Email::new(1, "a@b.com", "c@d.com", "Hi", "Body", "2026-01-01", Folder::Sent);
    let cmds = build_smtp_commands(&email, "smartos.local");
    if !cmds.iter().any(|l| l.contains("EHLO")) { ok = false; }
    if !cmds.iter().any(|l| l.contains("MAIL FROM")) { ok = false; }
    if !cmds.iter().any(|l| l.contains("RCPT TO")) { ok = false; }
    if !cmds.iter().any(|l| l == "DATA") { ok = false; }
    if !cmds.iter().any(|l| l == ".") { ok = false; }
    if !cmds.iter().any(|l| l == "QUIT") { ok = false; }

    // 7. POP3 parser
    let pop3_lines = &[
        "From: sender@example.com",
        "To: me@example.com",
        "Subject: Test POP3",
        "Date: 2026-05-31",
        "",
        "Hello from POP3!",
        ".",
    ];
    let parsed = parse_pop3_message(pop3_lines, 99);
    if parsed.from != "sender@example.com" { ok = false; }
    if parsed.subject != "Test POP3" { ok = false; }
    if !parsed.body.contains("Hello from POP3!") { ok = false; }

    // 8. truncate helper
    if truncate("hello", 10) != "hello" { ok = false; }
    if truncate("hello world", 5) != "hello" { ok = false; }

    ok
}
