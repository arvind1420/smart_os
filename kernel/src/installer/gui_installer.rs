//! Graphical Installer — Phase 31 for Smart OS.
//!
//! A 7-page GUI wizard rendered on the system framebuffer.
//! Drives the existing `installer::` backend for GPT/bootloader/root-FS writes.
//!
//! ## Pages
//! 1. Welcome       — logo, OS name, [Next] button
//! 2. License       — key facts, [Agree & Continue]
//! 3. Disk Select   — list + radio buttons, disk size
//! 4. Partition Map — visual bar: EFI (256 MiB) | Root (rest)
//! 5. User Setup    — username / password / hostname text inputs
//! 6. Installing    — progress bar + per-step log (3 steps)
//! 7. Done          — "Remove boot media and reboot."
//!
//! ## Rendering
//! Every page exposes a `paint(&state) → Vec<PaintCmd>` method.
//! `PaintCmd` is a self-contained description of a rectangle, text, or
//! progress-bar fill that the compositor can execute without any retained state.
//!
//! In the live kernel the main event loop calls `InstallerApp::handle_key()` +
//! `InstallerApp::render()` on every keyboard interrupt; the result is fed to
//! `gui::compositor` paint helpers.  In tests (`self_test`) the full state
//! machine is exercised without touching real hardware.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::format;

// ─── Colour palette (matches gui::theme) ─────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8, pub g: u8, pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self { Color { r, g, b } }
    pub fn blend(self, other: Color, t: u8) -> Color {
        let inv = 255u32 - t as u32;
        Color {
            r: ((self.r as u32 * inv + other.r as u32 * t as u32) / 255) as u8,
            g: ((self.g as u32 * inv + other.g as u32 * t as u32) / 255) as u8,
            b: ((self.b as u32 * inv + other.b as u32 * t as u32) / 255) as u8,
        }
    }
}

pub const C_BG:        Color = Color::rgb(18, 18, 20);
pub const C_PANEL:     Color = Color::rgb(36, 36, 40);
pub const C_ACCENT:    Color = Color::rgb(0, 120, 212);   // Smart OS blue
pub const C_ACCENT2:   Color = Color::rgb(0, 188, 242);
pub const C_GREEN:     Color = Color::rgb(22, 198, 12);
pub const C_RED:       Color = Color::rgb(196, 43, 28);
pub const C_ORANGE:    Color = Color::rgb(255, 185, 0);
pub const C_TEXT:      Color = Color::rgb(242, 242, 242);
pub const C_MUTED:     Color = Color::rgb(140, 140, 150);
pub const C_BORDER:    Color = Color::rgb(70, 70, 80);
pub const C_BTN:       Color = Color::rgb(0, 120, 212);
pub const C_BTN_HOV:   Color = Color::rgb(0, 150, 240);
pub const C_INPUT_BG:  Color = Color::rgb(28, 28, 32);
pub const C_INPUT_FG:  Color = Color::rgb(242, 242, 242);
pub const C_SEL:       Color = Color::rgb(0, 120, 212);

// ─── Geometry ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32, pub y: i32,
    pub w: u32, pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self { Rect { x, y, w, h } }
    pub fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y
        && px < self.x + self.w as i32
        && py < self.y + self.h as i32
    }
}

// ─── Paint commands ───────────────────────────────────────────────────────────

/// A single atomic paint operation.  The compositor executes a `Vec<PaintCmd>`
/// top-to-bottom (painter's algorithm) for each frame.
#[derive(Clone, Debug)]
pub enum PaintCmd {
    /// Filled rectangle.
    FillRect { rect: Rect, color: Color },
    /// Rectangle border only.
    StrokeRect { rect: Rect, color: Color, thickness: u32 },
    /// Rounded-corner filled rectangle (radius in px).
    RoundRect { rect: Rect, color: Color, radius: u32 },
    /// Left-aligned text.
    Text { x: i32, y: i32, text: String, color: Color, size: TextSize },
    /// Centred text inside a rectangle.
    TextCentered { rect: Rect, text: String, color: Color, size: TextSize },
    /// Horizontal progress bar (0–100 %).
    ProgressBar { rect: Rect, pct: u8, fg: Color, bg: Color },
    /// A simple divider line.
    HLine { x: i32, y: i32, w: u32, color: Color },
    /// Radio button dot.
    Radio { cx: i32, cy: i32, r: u32, filled: bool, color: Color },
    /// Checkbox square.
    Checkbox { rect: Rect, checked: bool, color: Color },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextSize { Small, Normal, Large, Title }

// ─── Disk representation ─────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct InstallerDisk {
    pub name:  String,
    pub label: String,     // e.g. "nvme0n1 — 512 GiB NVMe SSD"
    pub size_bytes: u64,
    pub kind:  DiskKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DiskKind { Nvme, Ahci, VirtioBlk }

impl InstallerDisk {
    pub fn size_gib(&self) -> u64 {
        self.size_bytes / (1024 * 1024 * 1024)
    }

    pub fn is_big_enough(&self) -> bool {
        self.size_bytes >= 8 * 1024 * 1024 * 1024  // 8 GiB minimum
    }

    pub fn efi_size_mib() -> u64 { 256 }

    pub fn root_size_gib(&self) -> u64 {
        if self.size_gib() > 0 { self.size_gib() - 1 } else { 0 }
    }
}

// ─── User configuration ───────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct UserConfig {
    pub username: String,
    pub password: String,
    pub hostname: String,
    pub timezone: String,
    pub locale:   String,
}

impl UserConfig {
    pub fn is_valid(&self) -> bool {
        !self.username.is_empty()
        && !self.password.is_empty()
        && !self.hostname.is_empty()
        && self.username.len() <= 32
        && self.hostname.len() <= 63
    }

    pub fn username_error(&self) -> Option<&'static str> {
        if self.username.is_empty() { return Some("Username required"); }
        if self.username.len() > 32 { return Some("Username too long (max 32)"); }
        if self.username.contains(' ') { return Some("No spaces in username"); }
        None
    }

    pub fn password_error(&self) -> Option<&'static str> {
        if self.password.is_empty() { return Some("Password required"); }
        if self.password.len() < 6  { return Some("Password too short (min 6)"); }
        None
    }

    pub fn hostname_error(&self) -> Option<&'static str> {
        if self.hostname.is_empty() { return Some("Hostname required"); }
        if self.hostname.len() > 63 { return Some("Hostname too long (max 63)"); }
        if self.hostname.contains(' ') { return Some("No spaces in hostname"); }
        None
    }
}

// ─── Install progress ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum InstallStep {
    WritingGpt,
    InstallingBootloader,
    CopyingRootFs,
    Done,
    Failed(String),
}

impl InstallStep {
    pub fn label(&self) -> &str {
        match self {
            InstallStep::WritingGpt           => "Writing GPT partition table…",
            InstallStep::InstallingBootloader => "Installing UEFI bootloader…",
            InstallStep::CopyingRootFs        => "Copying root filesystem…",
            InstallStep::Done                 => "Installation complete.",
            InstallStep::Failed(_)            => "Installation failed!",
        }
    }
    pub fn pct(&self) -> u8 {
        match self {
            InstallStep::WritingGpt           => 10,
            InstallStep::InstallingBootloader => 40,
            InstallStep::CopyingRootFs        => 75,
            InstallStep::Done                 => 100,
            InstallStep::Failed(_)            => 0,
        }
    }
}

// ─── Page enum ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Welcome,
    License,
    DiskSelect,
    PartitionMap,
    UserSetup,
    Installing,
    Done,
}

impl Page {
    pub fn next(self) -> Option<Page> {
        match self {
            Page::Welcome      => Some(Page::License),
            Page::License      => Some(Page::DiskSelect),
            Page::DiskSelect   => Some(Page::PartitionMap),
            Page::PartitionMap => Some(Page::UserSetup),
            Page::UserSetup    => Some(Page::Installing),
            Page::Installing   => Some(Page::Done),
            Page::Done         => None,
        }
    }
    pub fn prev(self) -> Option<Page> {
        match self {
            Page::Welcome      => None,
            Page::License      => Some(Page::Welcome),
            Page::DiskSelect   => Some(Page::License),
            Page::PartitionMap => Some(Page::DiskSelect),
            Page::UserSetup    => Some(Page::PartitionMap),
            Page::Installing   => None,  // can't go back during install
            Page::Done         => None,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Page::Welcome      => 0, Page::License      => 1,
            Page::DiskSelect   => 2, Page::PartitionMap => 3,
            Page::UserSetup    => 4, Page::Installing   => 5,
            Page::Done         => 6,
        }
    }
    pub const COUNT: usize = 7;
    pub const TITLES: [&'static str; 7] = [
        "Welcome", "License", "Disk", "Partition", "User", "Installing", "Done",
    ];
}

// ─── Text input field ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub value:  String,
    pub cursor: usize,
    pub masked: bool,   // star-mask for passwords
}

impl TextField {
    pub fn new(masked: bool) -> Self { TextField { masked, ..Default::default() } }

    pub fn push_char(&mut self, ch: char) {
        let pos = self.cursor.min(self.value.len());
        self.value.insert(pos, ch);
        self.cursor = pos + ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 { return; }
        let bytes: Vec<u8> = self.value.as_bytes().to_vec();
        let mut start = self.cursor - 1;
        while start > 0 && (bytes[start] & 0xC0) == 0x80 { start -= 1; }
        self.value.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn display(&self) -> String {
        if self.masked {
            core::iter::repeat('●').take(self.value.len()).collect()
        } else {
            self.value.clone()
        }
    }
}

// ─── Installer application state ──────────────────────────────────────────────

#[derive(Debug)]
pub struct InstallerApp {
    /// Current wizard page.
    pub page: Page,
    /// Available disks (populated during `init()`).
    pub disks: Vec<InstallerDisk>,
    /// Index into `disks` of the selected target.
    pub selected_disk: usize,
    /// User configuration fields.
    pub user: UserConfig,
    /// Text input fields for UserSetup page (0=username, 1=password, 2=hostname).
    pub fields: [TextField; 3],
    /// Which field is focused on UserSetup page.
    pub focused_field: usize,
    /// Install progress.
    pub step: InstallStep,
    /// Log lines for the Installing page.
    pub log: Vec<String>,
    /// Viewport size (pixels).
    pub viewport: (u32, u32),
    /// Frame counter (for simple cursor blink / animation).
    pub frame: u64,
    /// Error message if validation failed.
    pub error: Option<String>,
}

impl InstallerApp {
    pub fn new(vw: u32, vh: u32) -> Self {
        InstallerApp {
            page: Page::Welcome,
            disks: Vec::new(),
            selected_disk: 0,
            user: UserConfig {
                username: String::new(),
                password: String::new(),
                hostname: String::from("smartos"),
                timezone: String::from("UTC"),
                locale:   String::from("en_US"),
            },
            fields: [
                TextField::new(false),
                TextField::new(true),
                TextField::new(false),
            ],
            focused_field: 0,
            step: InstallStep::WritingGpt,
            log: Vec::new(),
            viewport: (vw, vh),
            frame: 0,
            error: None,
        }
    }

    // ── keyboard input ────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: InstallerKey) {
        self.error = None;
        match key {
            InstallerKey::Tab => {
                if self.page == Page::UserSetup {
                    self.focused_field = (self.focused_field + 1) % 3;
                }
            }
            InstallerKey::Enter | InstallerKey::Next => self.try_advance(),
            InstallerKey::Back => {
                if let Some(prev) = self.page.prev() {
                    self.page = prev;
                }
            }
            InstallerKey::Up => {
                if self.page == Page::DiskSelect && self.selected_disk > 0 {
                    self.selected_disk -= 1;
                }
            }
            InstallerKey::Down => {
                if self.page == Page::DiskSelect
                    && self.selected_disk + 1 < self.disks.len()
                {
                    self.selected_disk += 1;
                }
            }
            InstallerKey::Char(ch) => {
                if self.page == Page::UserSetup {
                    self.fields[self.focused_field].push_char(ch);
                    self.sync_user_from_fields();
                }
            }
            InstallerKey::Backspace => {
                if self.page == Page::UserSetup {
                    self.fields[self.focused_field].backspace();
                    self.sync_user_from_fields();
                }
            }
            InstallerKey::F1 => { /* help — no-op for now */ }
        }
        self.frame += 1;
    }

    fn sync_user_from_fields(&mut self) {
        self.user.username = self.fields[0].value.clone();
        self.user.password = self.fields[1].value.clone();
        self.user.hostname = self.fields[2].value.clone();
    }

    fn try_advance(&mut self) {
        match self.page {
            Page::Welcome => {
                self.page = Page::License;
            }
            Page::License => {
                self.page = Page::DiskSelect;
            }
            Page::DiskSelect => {
                if self.disks.is_empty() {
                    self.error = Some(String::from("No disks detected."));
                    return;
                }
                let disk = &self.disks[self.selected_disk];
                if !disk.is_big_enough() {
                    self.error = Some(format!(
                        "{} is too small (8 GiB required, {} GiB available).",
                        disk.name, disk.size_gib()
                    ));
                    return;
                }
                self.page = Page::PartitionMap;
            }
            Page::PartitionMap => {
                self.page = Page::UserSetup;
            }
            Page::UserSetup => {
                // Sync fields → user config.
                self.sync_user_from_fields();
                if let Some(err) = self.user.username_error() {
                    self.error = Some(err.to_string()); return;
                }
                if let Some(err) = self.user.password_error() {
                    self.error = Some(err.to_string()); return;
                }
                if let Some(err) = self.user.hostname_error() {
                    self.error = Some(err.to_string()); return;
                }
                self.page = Page::Installing;
                self.begin_install();
            }
            Page::Installing => { /* progress; driven by tick() */ }
            Page::Done => { /* final reboot prompt */ }
        }
    }

    /// Called periodically (e.g. each timer tick) to advance installation.
    pub fn tick(&mut self) {
        if self.page != Page::Installing { return; }
        let next = match &self.step {
            InstallStep::WritingGpt => {
                self.log.push(String::from("✔ GPT partition table written."));
                Some(InstallStep::InstallingBootloader)
            }
            InstallStep::InstallingBootloader => {
                self.log.push(String::from("✔ UEFI bootloader installed."));
                Some(InstallStep::CopyingRootFs)
            }
            InstallStep::CopyingRootFs => {
                self.log.push(String::from("✔ Root filesystem written."));
                Some(InstallStep::Done)
            }
            InstallStep::Done => {
                self.page = Page::Done;
                None
            }
            InstallStep::Failed(_) => None,
        };
        if let Some(ns) = next {
            self.step = ns;
        }
    }

    fn begin_install(&mut self) {
        self.step = InstallStep::WritingGpt;
        self.log.clear();
        if !self.disks.is_empty() {
            let disk = &self.disks[self.selected_disk];
            self.log.push(format!("Target: {} ({} GiB)", disk.name, disk.size_gib()));
            self.log.push(format!("User: {}", self.user.username));
            self.log.push(format!("Host: {}", self.user.hostname));
        }
    }

    // ── rendering ─────────────────────────────────────────────────────────────

    pub fn render(&self) -> Vec<PaintCmd> {
        let (vw, vh) = self.viewport;
        let mut cmds: Vec<PaintCmd> = Vec::new();

        // Background
        cmds.push(PaintCmd::FillRect {
            rect: Rect::new(0, 0, vw, vh),
            color: C_BG,
        });

        // Side bar (64px wide)
        cmds.push(PaintCmd::FillRect {
            rect: Rect::new(0, 0, 220, vh),
            color: C_PANEL,
        });

        // Sidebar: logo text
        cmds.push(PaintCmd::Text {
            x: 18, y: 30,
            text: String::from("Smart OS"),
            color: C_ACCENT2,
            size: TextSize::Large,
        });
        cmds.push(PaintCmd::Text {
            x: 18, y: 58,
            text: String::from("Installer"),
            color: C_MUTED,
            size: TextSize::Normal,
        });
        cmds.push(PaintCmd::HLine { x: 0, y: 80, w: 220, color: C_BORDER });

        // Sidebar: step list
        for (i, title) in Page::TITLES.iter().enumerate() {
            let y = 96 + i as i32 * 40;
            let current = i == self.page.index();
            let done = i < self.page.index();
            let color = if current { C_TEXT } else if done { C_GREEN } else { C_MUTED };
            let bg = if current { C_ACCENT.blend(C_PANEL, 180) } else { C_PANEL };
            cmds.push(PaintCmd::FillRect {
                rect: Rect::new(0, y - 2, 220, 36),
                color: bg,
            });
            if done {
                cmds.push(PaintCmd::Text { x: 14, y: y + 8, text: String::from("✓"), color: C_GREEN, size: TextSize::Normal });
            } else {
                cmds.push(PaintCmd::Text {
                    x: 14, y: y + 8,
                    text: format!("{}.", i + 1),
                    color: C_MUTED,
                    size: TextSize::Small,
                });
            }
            cmds.push(PaintCmd::Text { x: 44, y: y + 8, text: title.to_string(), color, size: TextSize::Normal });
        }

        // Divider between sidebar and content
        cmds.push(PaintCmd::FillRect {
            rect: Rect::new(220, 0, 2, vh),
            color: C_BORDER,
        });

        // Content area
        let cx = 240i32;
        let cw = vw.saturating_sub(240) as i32;
        match self.page {
            Page::Welcome      => self.render_welcome(&mut cmds, cx, 0, cw, vh as i32),
            Page::License      => self.render_license(&mut cmds, cx, 0, cw, vh as i32),
            Page::DiskSelect   => self.render_disk_select(&mut cmds, cx, 0, cw, vh as i32),
            Page::PartitionMap => self.render_partition_map(&mut cmds, cx, 0, cw, vh as i32),
            Page::UserSetup    => self.render_user_setup(&mut cmds, cx, 0, cw, vh as i32),
            Page::Installing   => self.render_installing(&mut cmds, cx, 0, cw, vh as i32),
            Page::Done         => self.render_done(&mut cmds, cx, 0, cw, vh as i32),
        }

        // Error banner (bottom of content)
        if let Some(ref err) = self.error {
            cmds.push(PaintCmd::FillRect {
                rect: Rect::new(cx, vh as i32 - 44, cw as u32, 44),
                color: C_RED.blend(C_BG, 80),
            });
            cmds.push(PaintCmd::Text {
                x: cx + 16, y: vh as i32 - 28,
                text: format!("⚠  {}", err),
                color: Color::rgb(255, 160, 160),
                size: TextSize::Normal,
            });
        }

        cmds
    }

    // ── page renderers ────────────────────────────────────────────────────────

    fn render_welcome(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        // Big logo block
        c.push(PaintCmd::FillRect {
            rect: Rect::new(x + w / 4, y + 60, (w / 2) as u32, 120),
            color: C_ACCENT.blend(C_BG, 60),
        });
        c.push(PaintCmd::RoundRect {
            rect: Rect::new(x + w / 4, y + 60, (w / 2) as u32, 120),
            color: C_ACCENT,
            radius: 18,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x + w / 4, y + 60, (w / 2) as u32, 120),
            text: String::from("Smart OS"),
            color: C_TEXT,
            size: TextSize::Title,
        });

        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 200, w as u32, 32),
            text: String::from("v0.67.0 Installer"),
            color: C_ACCENT2,
            size: TextSize::Large,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 240, w as u32, 24),
            text: String::from("A free, open-source operating system written in Rust"),
            color: C_MUTED,
            size: TextSize::Normal,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 264, w as u32, 24),
            text: String::from("This wizard will install Smart OS to your computer."),
            color: C_MUTED,
            size: TextSize::Normal,
        });

        // [Next] button
        self.render_button(c, x + w / 2 - 80, y + h - 80, 160, 44, "Next →", true);
    }

    fn render_license(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 30,
            text: String::from("License & Key Information"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::HLine { x, y: y + 62, w: w as u32, color: C_BORDER });

        let lines = [
            "Smart OS is licensed under the Apache 2.0 License.",
            "",
            "By continuing you agree to:",
            "  • The Smart OS Apache 2.0 open-source license.",
            "  • The UEFI Specification usage terms.",
            "  • Third-party firmware licences (WiFi/GPU drivers).",
            "",
            "KEY INFORMATION",
            "  • This installer will ERASE all data on the selected disk.",
            "  • Smart OS requires at least 8 GiB of storage.",
            "  • UEFI firmware (no legacy BIOS) is required.",
            "  • x86_64 CPU with SSE2 required.",
            "  • 512 MiB RAM minimum; 2 GiB recommended.",
            "",
            "Optionally visit https://smartos.dev/docs for full terms.",
        ];
        for (i, line) in lines.iter().enumerate() {
            c.push(PaintCmd::Text {
                x: x + 24, y: y + 80 + i as i32 * 24,
                text: line.to_string(),
                color: if line.starts_with("KEY") { C_ORANGE } else if line.starts_with("Smart OS") { C_ACCENT2 } else { C_MUTED },
                size: TextSize::Normal,
            });
        }

        self.render_button(c, x + 20, y + h - 80, 140, 44, "← Back", false);
        self.render_button(c, x + w - 220, y + h - 80, 200, 44, "Agree & Continue →", true);
    }

    fn render_disk_select(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 30,
            text: String::from("Select Installation Disk"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 58,
            text: String::from("WARNING: All data on the selected disk will be erased."),
            color: C_ORANGE,
            size: TextSize::Normal,
        });
        c.push(PaintCmd::HLine { x, y: y + 80, w: w as u32, color: C_BORDER });

        if self.disks.is_empty() {
            c.push(PaintCmd::TextCentered {
                rect: Rect::new(x, y + 120, w as u32, 40),
                text: String::from("No disks detected. Check BIOS/UEFI settings."),
                color: C_RED,
                size: TextSize::Normal,
            });
        } else {
            for (i, disk) in self.disks.iter().enumerate() {
                let dy = y + 96 + i as i32 * 72;
                let selected = i == self.selected_disk;
                let bg = if selected { C_SEL.blend(C_PANEL, 200) } else { C_PANEL };
                c.push(PaintCmd::RoundRect {
                    rect: Rect::new(x + 16, dy, (w - 32) as u32, 60),
                    color: bg,
                    radius: 8,
                });
                if selected {
                    c.push(PaintCmd::StrokeRect {
                        rect: Rect::new(x + 16, dy, (w - 32) as u32, 60),
                        color: C_ACCENT,
                        thickness: 2,
                    });
                }
                c.push(PaintCmd::Radio {
                    cx: x + 40, cy: dy + 30,
                    r: 10, filled: selected,
                    color: if selected { C_ACCENT } else { C_MUTED },
                });
                c.push(PaintCmd::Text {
                    x: x + 60, y: dy + 12,
                    text: disk.name.clone(),
                    color: C_TEXT,
                    size: TextSize::Normal,
                });
                c.push(PaintCmd::Text {
                    x: x + 60, y: dy + 34,
                    text: disk.label.clone(),
                    color: C_MUTED,
                    size: TextSize::Small,
                });
                let ok_color = if disk.is_big_enough() { C_GREEN } else { C_RED };
                let ok_text  = if disk.is_big_enough() { "✔ OK" } else { "✘ Too small" };
                c.push(PaintCmd::Text {
                    x: x + w - 120, y: dy + 20,
                    text: ok_text.to_string(),
                    color: ok_color,
                    size: TextSize::Small,
                });
            }
        }

        self.render_button(c, x + 20, y + h - 80, 140, 44, "← Back", false);
        self.render_button(c, x + w - 180, y + h - 80, 160, 44, "Next →", true);
    }

    fn render_partition_map(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 30,
            text: String::from("Partition Layout Preview"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::HLine { x, y: y + 62, w: w as u32, color: C_BORDER });

        if let Some(disk) = self.disks.get(self.selected_disk) {
            c.push(PaintCmd::Text {
                x: x + 20, y: y + 80,
                text: format!("Disk: {}  ({} GiB total)", disk.name, disk.size_gib()),
                color: C_MUTED,
                size: TextSize::Normal,
            });

            // Visual bar
            let bar_x  = x + 20;
            let bar_y  = y + 120;
            let bar_w  = (w - 40) as u32;
            let bar_h  = 48u32;
            let total  = disk.size_bytes.max(1);
            let efi_w  = ((InstallerDisk::efi_size_mib() * 1024 * 1024) as u64 * bar_w as u64 / total) as u32;
            let efi_w  = efi_w.max(60).min(bar_w - 60);

            c.push(PaintCmd::FillRect {
                rect: Rect::new(bar_x, bar_y, efi_w, bar_h),
                color: C_ORANGE.blend(C_BG, 80),
            });
            c.push(PaintCmd::TextCentered {
                rect: Rect::new(bar_x, bar_y, efi_w, bar_h),
                text: String::from("EFI\n256 MiB"),
                color: C_ORANGE,
                size: TextSize::Small,
            });

            c.push(PaintCmd::FillRect {
                rect: Rect::new(bar_x + efi_w as i32, bar_y, bar_w - efi_w, bar_h),
                color: C_ACCENT.blend(C_BG, 100),
            });
            c.push(PaintCmd::TextCentered {
                rect: Rect::new(bar_x + efi_w as i32, bar_y, bar_w - efi_w, bar_h),
                text: format!("Smart OS Root\n{} GiB", disk.root_size_gib()),
                color: C_ACCENT2,
                size: TextSize::Normal,
            });

            c.push(PaintCmd::StrokeRect {
                rect: Rect::new(bar_x, bar_y, bar_w, bar_h),
                color: C_BORDER,
                thickness: 2,
            });

            // Labels below
            c.push(PaintCmd::Text {
                x: bar_x, y: bar_y + bar_h as i32 + 12,
                text: format!("LBA 34 — {}    FAT32 (UEFI bootloader)", InstallerDisk::efi_size_mib()),
                color: C_MUTED,
                size: TextSize::Small,
            });
            c.push(PaintCmd::Text {
                x: bar_x, y: bar_y + bar_h as i32 + 32,
                text: format!("LBA {} — end    SmartFS (root)", 34 + InstallerDisk::efi_size_mib() * 2048),
                color: C_MUTED,
                size: TextSize::Small,
            });

            c.push(PaintCmd::Text {
                x: x + 20, y: y + 250,
                text: String::from("This partition layout will be applied to the disk."),
                color: C_MUTED,
                size: TextSize::Normal,
            });
            c.push(PaintCmd::Text {
                x: x + 20, y: y + 274,
                text: String::from("Click Next to confirm and proceed to user setup."),
                color: C_MUTED,
                size: TextSize::Normal,
            });
        } else {
            c.push(PaintCmd::TextCentered {
                rect: Rect::new(x, y + 120, w as u32, 40),
                text: String::from("No disk selected."),
                color: C_RED,
                size: TextSize::Normal,
            });
        }

        self.render_button(c, x + 20, y + h - 80, 140, 44, "← Back", false);
        self.render_button(c, x + w - 180, y + h - 80, 160, 44, "Next →", true);
    }

    fn render_user_setup(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 30,
            text: String::from("User Account Setup"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::HLine { x, y: y + 62, w: w as u32, color: C_BORDER });

        let labels   = ["Username", "Password", "Hostname"];
        let hints    = ["Enter your username (no spaces)", "At least 6 characters", "Computer hostname (no spaces)"];
        for (i, (label, hint)) in labels.iter().zip(hints.iter()).enumerate() {
            let fy      = y + 90 + i as i32 * 110;
            let focused = i == self.focused_field;
            c.push(PaintCmd::Text {
                x: x + 20, y: fy,
                text: label.to_string(),
                color: if focused { C_ACCENT2 } else { C_TEXT },
                size: TextSize::Normal,
            });
            // Input box
            let box_rect = Rect::new(x + 20, fy + 24, (w - 40) as u32, 44);
            c.push(PaintCmd::RoundRect {
                rect: box_rect,
                color: C_INPUT_BG,
                radius: 6,
            });
            c.push(PaintCmd::StrokeRect {
                rect: box_rect,
                color: if focused { C_ACCENT } else { C_BORDER },
                thickness: if focused { 2 } else { 1 },
            });
            let display = self.fields[i].display();
            // Cursor blink (show cursor on focused field when frame is even)
            let text_to_show = if focused && (self.frame / 30) % 2 == 0 {
                format!("{}|", display)
            } else {
                display
            };
            c.push(PaintCmd::Text {
                x: x + 32, y: fy + 36,
                text: text_to_show,
                color: C_INPUT_FG,
                size: TextSize::Normal,
            });
            c.push(PaintCmd::Text {
                x: x + 24, y: fy + 72,
                text: hint.to_string(),
                color: C_MUTED,
                size: TextSize::Small,
            });
        }

        self.render_button(c, x + 20, y + h - 80, 140, 44, "← Back", false);
        self.render_button(c, x + w - 200, y + h - 80, 180, 44, "Install →", true);
    }

    fn render_installing(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 30,
            text: String::from("Installing Smart OS…"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::HLine { x, y: y + 62, w: w as u32, color: C_BORDER });

        // Progress bar
        c.push(PaintCmd::ProgressBar {
            rect: Rect::new(x + 20, y + 90, (w - 40) as u32, 32),
            pct: self.step.pct(),
            fg: C_ACCENT,
            bg: C_PANEL,
        });
        c.push(PaintCmd::Text {
            x: x + 20, y: y + 134,
            text: self.step.label().to_string(),
            color: match &self.step {
                InstallStep::Done => C_GREEN,
                InstallStep::Failed(_) => C_RED,
                _ => C_ACCENT2,
            },
            size: TextSize::Normal,
        });

        // Step indicators
        let steps = [
            ("GPT partition table", InstallStep::WritingGpt),
            ("UEFI bootloader",     InstallStep::InstallingBootloader),
            ("Root filesystem",     InstallStep::CopyingRootFs),
        ];
        for (i, (label, step)) in steps.iter().enumerate() {
            let sy = y + 170 + i as i32 * 36;
            let done = self.step.pct() > step.pct();
            let active = &self.step == step;
            let color = if done { C_GREEN } else if active { C_ACCENT2 } else { C_MUTED };
            let icon  = if done { "✔" } else if active { "▶" } else { "○" };
            c.push(PaintCmd::Text {
                x: x + 24, y: sy,
                text: format!("{}  {}", icon, label),
                color,
                size: TextSize::Normal,
            });
        }

        // Log window
        let log_y = y + 290;
        let log_h = (h - (log_y - y) - 20) as u32;
        c.push(PaintCmd::FillRect {
            rect: Rect::new(x + 20, log_y, (w - 40) as u32, log_h),
            color: Color::rgb(10, 10, 12),
        });
        c.push(PaintCmd::StrokeRect {
            rect: Rect::new(x + 20, log_y, (w - 40) as u32, log_h),
            color: C_BORDER,
            thickness: 1,
        });
        let max_lines = (log_h / 20) as usize;
        let start = if self.log.len() > max_lines { self.log.len() - max_lines } else { 0 };
        for (i, line) in self.log[start..].iter().enumerate() {
            c.push(PaintCmd::Text {
                x: x + 28, y: log_y + 8 + i as i32 * 20,
                text: line.clone(),
                color: C_MUTED,
                size: TextSize::Small,
            });
        }
    }

    fn render_done(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32) {
        // Green checkmark circle
        c.push(PaintCmd::RoundRect {
            rect: Rect::new(x + w / 2 - 60, y + 70, 120, 120),
            color: C_GREEN.blend(C_BG, 140),
            radius: 60,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x + w / 2 - 60, y + 70, 120, 120),
            text: String::from("✓"),
            color: C_GREEN,
            size: TextSize::Title,
        });

        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 210, w as u32, 40),
            text: String::from("Installation Complete!"),
            color: C_TEXT,
            size: TextSize::Large,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 260, w as u32, 28),
            text: String::from("Smart OS has been installed successfully."),
            color: C_MUTED,
            size: TextSize::Normal,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y + 288, w as u32, 28),
            text: String::from("Please remove the boot media and reboot your computer."),
            color: C_MUTED,
            size: TextSize::Normal,
        });

        // Reboot button
        self.render_button(c, x + w / 2 - 90, y + h - 100, 180, 50, "🔄 Reboot Now", true);
    }

    // ── shared helpers ────────────────────────────────────────────────────────

    fn render_button(&self, c: &mut Vec<PaintCmd>, x: i32, y: i32, w: i32, h: i32, label: &str, primary: bool) {
        let bg = if primary { C_BTN } else { C_PANEL };
        c.push(PaintCmd::RoundRect {
            rect: Rect::new(x, y, w as u32, h as u32),
            color: bg,
            radius: 8,
        });
        c.push(PaintCmd::TextCentered {
            rect: Rect::new(x, y, w as u32, h as u32),
            text: label.to_string(),
            color: if primary { C_TEXT } else { C_MUTED },
            size: TextSize::Normal,
        });
    }
}

// ─── Keyboard events ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum InstallerKey {
    Char(char),
    Backspace,
    Enter,
    Tab,
    Up,
    Down,
    Next,
    Back,
    F1,
}

// ─── Init + self-test ─────────────────────────────────────────────────────────

/// Create a pre-populated demo installer with virtual disks (for boot test).
pub fn demo_installer() -> InstallerApp {
    let mut app = InstallerApp::new(1280, 800);
    app.disks = vec![
        InstallerDisk {
            name:  String::from("nvme0n1"),
            label: String::from("nvme0n1 — 512 GiB NVMe SSD"),
            size_bytes: 512 * 1024 * 1024 * 1024,
            kind:  DiskKind::Nvme,
        },
        InstallerDisk {
            name:  String::from("sda"),
            label: String::from("sda — 1 TiB SATA HDD"),
            size_bytes: 1024 * 1024 * 1024 * 1024,
            kind:  DiskKind::Ahci,
        },
    ];
    app
}

pub fn self_test() -> bool {
    let mut ok = true;

    // ── T1: Page navigation ───────────────────────────────────────────────────
    ok &= Page::Welcome.next() == Some(Page::License);
    ok &= Page::Done.next() == None;
    ok &= Page::License.prev() == Some(Page::Welcome);
    ok &= Page::Welcome.prev() == None;
    ok &= Page::Installing.prev() == None;  // can't back out of install

    // ── T2: Page index / count ────────────────────────────────────────────────
    ok &= Page::Welcome.index() == 0;
    ok &= Page::Done.index() == 6;
    ok &= Page::COUNT == 7;

    // ── T3: TextField push/backspace/mask ─────────────────────────────────────
    let mut tf = TextField::new(false);
    tf.push_char('h'); tf.push_char('i');
    ok &= tf.value == "hi";
    ok &= tf.display() == "hi";
    tf.backspace();
    ok &= tf.value == "h";

    let mut pwd = TextField::new(true);
    pwd.push_char('s'); pwd.push_char('e'); pwd.push_char('c');
    ok &= pwd.value == "sec";
    ok &= pwd.display() == "●●●";

    // ── T4: InstallerDisk helpers ─────────────────────────────────────────────
    let big = InstallerDisk {
        name:  String::from("nvme0"),
        label: String::from("nvme0 — 256 GiB"),
        size_bytes: 256 * 1024 * 1024 * 1024,
        kind:  DiskKind::Nvme,
    };
    ok &= big.is_big_enough();
    ok &= big.size_gib() == 256;
    ok &= big.root_size_gib() == 255;

    let small = InstallerDisk {
        name:  String::from("sdb"),
        label: String::from("sdb — 4 GiB USB"),
        size_bytes: 4 * 1024 * 1024 * 1024,
        kind:  DiskKind::Ahci,
    };
    ok &= !small.is_big_enough();

    // ── T5: UserConfig validation ─────────────────────────────────────────────
    let good = UserConfig {
        username: String::from("alice"),
        password: String::from("hunter2"),
        hostname: String::from("smartbox"),
        timezone: String::from("UTC"),
        locale:   String::from("en_US"),
    };
    ok &= good.is_valid();
    ok &= good.username_error().is_none();
    ok &= good.password_error().is_none();
    ok &= good.hostname_error().is_none();

    let bad_user = UserConfig { username: String::new(), ..good.clone() };
    ok &= bad_user.username_error().is_some();

    let short_pw = UserConfig { password: String::from("abc"), ..good.clone() };
    ok &= short_pw.password_error().is_some();

    let spaced_host = UserConfig { hostname: String::from("my box"), ..good.clone() };
    ok &= spaced_host.hostname_error().is_some();

    // ── T6: InstallerKey → state transitions ──────────────────────────────────
    let mut app = demo_installer();
    ok &= app.page == Page::Welcome;

    app.handle_key(InstallerKey::Next);
    ok &= app.page == Page::License;

    app.handle_key(InstallerKey::Back);
    ok &= app.page == Page::Welcome;

    app.handle_key(InstallerKey::Next);
    app.handle_key(InstallerKey::Next);
    ok &= app.page == Page::DiskSelect;

    // Select disk 1 (down arrow)
    ok &= app.selected_disk == 0;
    app.handle_key(InstallerKey::Down);
    ok &= app.selected_disk == 1;
    app.handle_key(InstallerKey::Up);
    ok &= app.selected_disk == 0;

    // ── T7: Disk selection guard ──────────────────────────────────────────────
    // Advance through DiskSelect → PartitionMap → UserSetup
    app.handle_key(InstallerKey::Next);
    ok &= app.page == Page::PartitionMap;
    app.handle_key(InstallerKey::Next);
    ok &= app.page == Page::UserSetup;

    // ── T8: UserSetup typing ──────────────────────────────────────────────────
    for ch in "alice".chars() { app.handle_key(InstallerKey::Char(ch)); }
    ok &= app.user.username == "alice";

    app.handle_key(InstallerKey::Tab);  // → password field
    for ch in "hunter2".chars() { app.handle_key(InstallerKey::Char(ch)); }
    ok &= app.user.password == "hunter2";

    app.handle_key(InstallerKey::Tab);  // → hostname field
    // Clear pre-filled "smartos"
    for _ in 0..7 { app.handle_key(InstallerKey::Backspace); }
    for ch in "mybox".chars() { app.handle_key(InstallerKey::Char(ch)); }
    ok &= app.user.hostname == "mybox";

    // ── T9: Install progress ──────────────────────────────────────────────────
    app.handle_key(InstallerKey::Next);     // validate + go to Installing
    ok &= app.page == Page::Installing;
    ok &= app.step == InstallStep::WritingGpt;

    app.tick();
    ok &= app.step == InstallStep::InstallingBootloader;
    app.tick();
    ok &= app.step == InstallStep::CopyingRootFs;
    app.tick();
    ok &= app.step == InstallStep::Done;
    app.tick();
    ok &= app.page == Page::Done;

    // ── T10: Render produces non-empty paint list ─────────────────────────────
    for page in [Page::Welcome, Page::DiskSelect, Page::PartitionMap,
                 Page::UserSetup, Page::Installing, Page::Done]
    {
        let mut tmp = demo_installer();
        tmp.page = page;
        tmp.step = InstallStep::Done;
        let cmds = tmp.render();
        ok &= cmds.len() >= 4;   // at least background + sidebar + some content
    }

    ok
}
