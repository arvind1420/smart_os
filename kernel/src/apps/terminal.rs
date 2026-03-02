/// Smart OS Terminal — Interactive shell application.
///
/// Provides a command-line interface with scrollable output,
/// editable input, and commands for VFS, AI, SmartFS, IPC, and system info.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

/// Terminal application state.
pub struct TerminalState {
    pub window_id: WindowId,
    /// Current working directory.
    pub cwd: String,
    /// Output lines: (text, color).
    pub output: Vec<(String, Color)>,
    /// Command history.
    pub cmd_history: Vec<String>,
    /// Whether state changed and needs GUI sync.
    pub dirty: bool,
}

pub static STATE: Mutex<Option<TerminalState>> = Mutex::new(None);

/// Terminal thread entry point.
pub fn run() {
    // Create the terminal window
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let mut win = Window::new("Terminal", 20, 35, 480, 340, ACCENT_GREEN);
        win.use_widgets = true;

        // Widget 0: Scrollable output area (top — most of the window)
        let output_widget = Widget::new(0, 0, 0, 480, 310,
            WidgetKind::ScrollText(ScrollableText::new(500)));

        // Widget 1: Text input (bottom — command line)
        let mut input_widget = Widget::new(1, 0, 314, 480, INPUT_HEIGHT,
            WidgetKind::TextInput(TextInput::new("Type a command...", ACCENT_GREEN)));
        input_widget.focused = true;

        win.add_widget(output_widget);
        win.add_widget(input_widget);
        win.focused_widget = Some(1);

        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Initialize terminal state
    let mut output = Vec::new();
    output.push((String::from("  ____  __  __    _    ____ _____    ___  ____"), ACCENT_CYAN));
    output.push((String::from(" / ___||  \\/  |  / \\  |  _ \\_   _|  / _ \\/ ___|"), ACCENT_CYAN));
    output.push((String::from(" \\___ \\| |\\/| | / _ \\ | |_) || |   | | | \\___ \\"), ACCENT_CYAN));
    output.push((String::from("  ___) | |  | |/ ___ \\|  _ < | |   | |_| |___) |"), ACCENT_CYAN));
    output.push((String::from(" |____/|_|  |_/_/   \\_\\_| \\_\\|_|    \\___/|____/"), ACCENT_CYAN));
    output.push((String::from(""), TEXT_PRIMARY));
    output.push((String::from("  Smart OS Terminal v0.4.0"), ACCENT_GREEN));
    output.push((String::from("  Type 'help' for available commands."), TEXT_SECONDARY));
    output.push((String::from(""), TEXT_PRIMARY));

    *STATE.lock() = Some(TerminalState {
        window_id,
        cwd: String::from("/"),
        output,
        cmd_history: Vec::new(),
        dirty: true,
    });

    // Main loop: poll for widget actions
    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            match action {
                WidgetAction::Execute(AppCommand::TextSubmitted(cmd)) => {
                    if !cmd.is_empty() {
                        let mut state = STATE.lock();
                        if let Some(ref mut s) = *state {
                            // Echo command with prompt
                            let prompt = format!("{}> {}", s.cwd, cmd);
                            s.output.push((prompt, ACCENT_GREEN));
                            s.cmd_history.push(cmd.clone());
                            // Execute
                            execute(&cmd, s);
                            s.output.push((String::new(), TEXT_PRIMARY)); // blank line
                            s.dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }

        // Yield CPU
        for _ in 0..5 {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Sync terminal state to window widgets (called from render loop).
pub fn sync_to_window(window: &mut Window) {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };
    if !s.dirty { return; }
    s.dirty = false;

    // Update the ScrollableText widget (id=0)
    if let Some(widget) = window.get_widget_mut(0) {
        if let WidgetKind::ScrollText(ref mut scroll) = widget.kind {
            scroll.lines = s.output.clone();
            scroll.scroll_to_bottom_pub();
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Command Execution
// ═══════════════════════════════════════════════════════════════

fn execute(cmd: &str, state: &mut TerminalState) {
    let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
    if parts.is_empty() { return; }

    match parts[0] {
        "help" => cmd_help(state),
        "clear" => { state.output.clear(); }
        "pwd" => {
            let cwd = state.cwd.clone();
            state.output.push((cwd, ACCENT_GREEN));
        }
        "ls" => cmd_ls(state, parts.get(1).copied()),
        "cd" => cmd_cd(state, parts.get(1).copied()),
        "cat" => cmd_cat(state, parts.get(1).copied()),
        "mkdir" => cmd_mkdir(state, parts.get(1).copied()),
        "echo" => cmd_echo(state, &parts[1..]),
        "write" => cmd_write(state, &parts[1..]),
        "tree" => cmd_tree(state, parts.get(1).copied()),
        "ps" => cmd_ps(state),
        "mem" => cmd_mem(state),
        "uptime" => cmd_uptime(state),
        "info" => cmd_info(state),
        "classify" => cmd_classify(state, parts.get(1).copied()),
        "store" => cmd_store(state, parts.get(1).copied()),
        "dedup" => cmd_dedup(state),
        "ports" => cmd_ports(state),
        "plugins" => cmd_plugins(state),
        // Phase 6: Networking commands
        "netinfo" => cmd_netinfo(state),
        "udpsend" => cmd_udpsend(state, &parts[1..]),
        // Phase 6: Disk commands
        "diskinfo" => cmd_diskinfo(state),
        "diskls" => cmd_diskls(state),
        "diskwrite" => cmd_diskwrite(state, &parts[1..]),
        "diskcat" => cmd_diskcat(state, parts.get(1).copied()),
        // Phase 6: SMP commands
        "cpus" => cmd_cpus(state),
        // Phase 7: Intelligent OS commands
        "predict" => cmd_predict(state),
        "kginfo" => cmd_kginfo(state),
        "kgquery" => cmd_kgquery(state, parts.get(1).copied()),
        "security" => cmd_security(state),
        "partinfo" => cmd_partinfo(state),
        // Phase 8: TCP, USB, FAT32, editor commands
        "tcpinfo" => cmd_tcpinfo(state),
        "tcpconnect" => cmd_tcpconnect(state, &parts[1..]),
        "tcpsend" => cmd_tcpsend(state, &parts[1..]),
        "tcprecv" => cmd_tcprecv(state, &parts[1..]),
        "tcpclose" => cmd_tcpclose(state, parts.get(1).copied()),
        "tcplisten" => cmd_tcplisten(state, parts.get(1).copied()),
        "usbinfo" => cmd_usbinfo(state),
        "fat32ls" => cmd_fat32ls(state, parts.get(1).copied()),
        "fat32cat" => cmd_fat32cat(state, parts.get(1).copied()),
        "edit" => cmd_edit(state, parts.get(1).copied()),
        // Phase 9: Desktop OS polish commands
        "date" => cmd_date(state),
        "hostname" => cmd_hostname(state),
        "whoami" => cmd_whoami(state),
        "env" => cmd_env(state),
        "export" => cmd_export(state, &parts[1..]),
        "kill" => cmd_kill(state, parts.get(1).copied()),
        "dns" => cmd_dns(state, parts.get(1).copied()),
        "calc" => cmd_calc(state),
        "taskmgr" => cmd_taskmgr(state),
        "settings" => cmd_settings(state),
        "notify" => cmd_notify(state, &parts[1..]),
        "clipboard" => cmd_clipboard(state),
        "version" => cmd_version(state),
        // Phase 10: Kernel Maturity commands
        "cowinfo" => cmd_cowinfo(state),
        "shmem" => cmd_shmem(state, &parts[1..]),
        "aslr" => cmd_aslr(state),
        "swapinfo" => cmd_swapinfo(state),
        "smpinfo" => cmd_smpinfo(state),
        "strace" => cmd_strace(state, parts.get(1).copied()),
        "sandbox" => cmd_sandbox(state),
        "gdbinfo" => cmd_gdbinfo(state),
        "workspace" => cmd_workspace(state, parts.get(1).copied()),
        "theme" => cmd_theme(state, parts.get(1).copied()),
        "ipv6info" => cmd_ipv6info(state),
        // Phase 11: Userland & Real Programs commands
        "exec" => cmd_exec(state, parts.get(1).copied()),
        "procs" => cmd_procs(state),
        "usrrun" => cmd_usrrun(state, parts.get(1).copied()),
        "fds" => cmd_fds(state, parts.get(1).copied()),
        "waitinfo" => cmd_waitinfo(state),
        "shutdown" => cmd_shutdown(state),
        "reboot" => cmd_reboot(state),
        _ => {
            state.output.push((format!("Unknown command: '{}'. Type 'help'.", parts[0]), ACCENT_RED));
        }
    }
}

fn cmd_help(state: &mut TerminalState) {
    let cmds = [
        ("help",            "Show this help message"),
        ("clear",           "Clear the terminal"),
        ("pwd",             "Print working directory"),
        ("ls [path]",       "List directory contents"),
        ("cd <dir>",        "Change directory"),
        ("cat <file>",      "Display file contents"),
        ("mkdir <dir>",     "Create a directory"),
        ("echo <text>",     "Print text"),
        ("write <f> <txt>", "Write text to a file"),
        ("tree [path]",     "Show directory tree"),
        ("ps",              "List running threads"),
        ("mem",             "Show heap memory usage"),
        ("uptime",          "Show system uptime"),
        ("info",            "Full system information"),
        ("classify <file>", "AI-classify a file"),
        ("store <file>",    "Store file via SmartFS"),
        ("dedup",           "Show SmartFS dedup stats"),
        ("ports",           "List IPC ports"),
        ("plugins",         "List active plugins"),
        ("netinfo",         "Show network interface info"),
        ("udpsend ip p msg","Send UDP packet"),
        ("diskinfo",        "Show disk device info"),
        ("diskls",          "List files on disk"),
        ("diskwrite f txt", "Write file to disk"),
        ("diskcat <file>",  "Read file from disk"),
        ("cpus",            "Show active CPU cores"),
        ("predict",         "Show predicted next apps"),
        ("kginfo",          "Knowledge graph stats"),
        ("kgquery <type>",  "Query KG nodes by type"),
        ("security",        "Anomaly monitor status"),
        ("partinfo",        "A/B partition status"),
        ("tcpinfo",         "Show TCP connections"),
        ("tcpconnect ip p", "Connect TCP to ip:port"),
        ("tcpsend id msg",  "Send data on TCP socket"),
        ("tcprecv id",      "Receive from TCP socket"),
        ("tcpclose id",     "Close TCP connection"),
        ("tcplisten port",  "Listen on TCP port"),
        ("usbinfo",         "Show USB devices"),
        ("fat32ls [path]",  "List FAT32 directory"),
        ("fat32cat <file>", "Read FAT32 file"),
        ("edit [file]",     "Open text editor"),
        ("date",            "Show current date/time"),
        ("hostname",        "Show system hostname"),
        ("whoami",          "Show current user"),
        ("env",             "List environment variables"),
        ("export K=V",      "Set environment variable"),
        ("kill <tid>",      "Kill a thread by TID"),
        ("dns <host>",      "Resolve hostname via DNS"),
        ("calc",            "Open Calculator"),
        ("taskmgr",         "Open Task Manager"),
        ("settings",        "Open Settings"),
        ("notify <msg>",    "Show a notification"),
        ("clipboard",       "Show clipboard contents"),
        ("version",         "Show OS version"),
        ("cowinfo",         "Copy-on-Write fork stats"),
        ("shmem [name sz]", "Shared memory info/create"),
        ("aslr",            "ASLR randomization info"),
        ("swapinfo",        "Swap space statistics"),
        ("smpinfo",         "SMP load balancing stats"),
        ("strace [tid]",    "Syscall tracing info"),
        ("sandbox",         "Sandbox/capabilities info"),
        ("gdbinfo",         "GDB debug stub status"),
        ("workspace [n]",   "Virtual desktop switch"),
        ("theme [name]",    "Switch color theme"),
        ("ipv6info",        "IPv6 network status"),
        ("exec <path>",     "Spawn user-space ELF process"),
        ("procs",           "Show process table"),
        ("usrrun <name>",   "Run /bin/<name> as user process"),
        ("fds [pid]",       "Show FDs for a process"),
        ("waitinfo",        "Show wait queue status"),
        ("shutdown",        "ACPI power off"),
        ("reboot",          "ACPI reboot"),
    ];
    state.output.push((String::from("Available commands:"), ACCENT_CYAN));
    for (cmd, desc) in cmds {
        state.output.push((format!("  {:16} {}", cmd, desc), TEXT_PRIMARY));
    }
}

fn resolve_path(cwd: &str, arg: &str) -> String {
    if arg.starts_with('/') {
        // Absolute path
        String::from(arg)
    } else if arg == ".." {
        // Parent directory
        if cwd == "/" { return String::from("/"); }
        match cwd.rfind('/') {
            Some(0) => String::from("/"),
            Some(pos) => String::from(&cwd[..pos]),
            None => String::from("/"),
        }
    } else if arg == "." {
        String::from(cwd)
    } else {
        // Relative path
        if cwd == "/" {
            format!("/{}", arg)
        } else {
            format!("{}/{}", cwd, arg)
        }
    }
}

fn cmd_ls(state: &mut TerminalState, path_arg: Option<&str>) {
    let path = match path_arg {
        Some(p) => resolve_path(&state.cwd, p),
        None => state.cwd.clone(),
    };
    match crate::vfs::readdir(&path) {
        Ok(entries) => {
            if entries.is_empty() {
                state.output.push((String::from("  (empty directory)"), TEXT_MUTED));
            } else {
                for entry in &entries {
                    let full_path = if path == "/" {
                        format!("/{}", entry)
                    } else {
                        format!("{}/{}", path, entry)
                    };
                    // Check if directory or file
                    let (icon, color) = match crate::vfs::readdir(&full_path) {
                        Ok(_) => ("[D]", ACCENT_CYAN),
                        Err(_) => ("[F]", ACCENT_GREEN),
                    };
                    // Get size if file
                    let size_str = if icon == "[F]" {
                        if let Ok(s) = crate::vfs::stat(&full_path) {
                            if let Some(size) = s.as_map().and_then(|m|
                                m.iter().find(|(k, _)| k.as_str() == Some("size"))
                                    .and_then(|(_, v)| v.as_u64())) {
                                if size >= 1024 {
                                    format!("  {}K", size / 1024)
                                } else {
                                    format!("  {}B", size)
                                }
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::from("/")
                    };
                    state.output.push((format!("  {} {}{}", icon, entry, size_str), color));
                }
            }
        }
        Err(e) => {
            state.output.push((format!("ls: {}: {}", path, e), ACCENT_RED));
        }
    }
}

fn cmd_cd(state: &mut TerminalState, dir_arg: Option<&str>) {
    let dir = match dir_arg {
        Some(d) => d,
        None => {
            state.cwd = String::from("/");
            return;
        }
    };
    let new_path = resolve_path(&state.cwd, dir);
    // Verify directory exists
    match crate::vfs::readdir(&new_path) {
        Ok(_) => { state.cwd = new_path; }
        Err(_) => {
            state.output.push((format!("cd: '{}': Not a directory", dir), ACCENT_RED));
        }
    }
}

fn cmd_cat(state: &mut TerminalState, file_arg: Option<&str>) {
    let file = match file_arg {
        Some(f) => f,
        None => {
            state.output.push((String::from("cat: missing file argument"), ACCENT_RED));
            return;
        }
    };
    let path = resolve_path(&state.cwd, file);
    match crate::vfs::open(&path) {
        Ok(fd) => {
            let mut buf = [0u8; 4096];
            match crate::vfs::read(fd, &mut buf) {
                Ok(n) => {
                    let text = core::str::from_utf8(&buf[..n]).unwrap_or("<binary data>");
                    for line in text.lines() {
                        state.output.push((String::from(line), TEXT_PRIMARY));
                    }
                }
                Err(e) => {
                    state.output.push((format!("cat: read error: {}", e), ACCENT_RED));
                }
            }
            crate::vfs::close(fd).ok();
        }
        Err(e) => {
            state.output.push((format!("cat: '{}': {}", path, e), ACCENT_RED));
        }
    }
}

fn cmd_mkdir(state: &mut TerminalState, dir_arg: Option<&str>) {
    let dir = match dir_arg {
        Some(d) => d,
        None => {
            state.output.push((String::from("mkdir: missing directory name"), ACCENT_RED));
            return;
        }
    };
    let path = resolve_path(&state.cwd, dir);
    match crate::vfs::mkdir(&path) {
        Ok(()) => state.output.push((format!("Created directory: {}", path), ACCENT_GREEN)),
        Err(e) => state.output.push((format!("mkdir: {}", e), ACCENT_RED)),
    }
}

fn cmd_echo(state: &mut TerminalState, args: &[&str]) {
    let text = args.join(" ");
    state.output.push((text, TEXT_PRIMARY));
}

fn cmd_write(state: &mut TerminalState, args: &[&str]) {
    if args.len() < 2 {
        state.output.push((String::from("write: usage: write <file> <content>"), ACCENT_RED));
        return;
    }
    let path = resolve_path(&state.cwd, args[0]);
    let content = args[1..].join(" ");
    match crate::vfs::create_and_write(&path, content.as_bytes()) {
        Ok(()) => state.output.push((format!("Wrote {} bytes to {}", content.len(), path), ACCENT_GREEN)),
        Err(e) => state.output.push((format!("write: {}", e), ACCENT_RED)),
    }
}

fn cmd_tree(state: &mut TerminalState, path_arg: Option<&str>) {
    let path = match path_arg {
        Some(p) => resolve_path(&state.cwd, p),
        None => state.cwd.clone(),
    };
    state.output.push((path.clone(), ACCENT_CYAN));
    tree_recursive(state, &path, String::new(), 0);
}

fn tree_recursive(state: &mut TerminalState, path: &str, prefix: String, depth: usize) {
    if depth > 5 { return; } // Limit recursion
    if let Ok(entries) = crate::vfs::readdir(path) {
        let count = entries.len();
        for (i, entry) in entries.iter().enumerate() {
            let is_last = i == count - 1;
            let connector = if is_last { "└── " } else { "├── " };
            let full_path = if path == "/" {
                format!("/{}", entry)
            } else {
                format!("{}/{}", path, entry)
            };
            let is_dir = crate::vfs::readdir(&full_path).is_ok();
            let color = if is_dir { ACCENT_CYAN } else { TEXT_PRIMARY };
            let suffix = if is_dir { "/" } else { "" };
            state.output.push((format!("{}{}{}{}", prefix, connector, entry, suffix), color));

            if is_dir {
                let child_prefix = if is_last {
                    format!("{}    ", prefix)
                } else {
                    format!("{}│   ", prefix)
                };
                tree_recursive(state, &full_path, child_prefix, depth + 1);
            }
        }
    }
}

fn cmd_ps(state: &mut TerminalState) {
    let threads = crate::process::scheduler::list_threads();
    state.output.push((format!("  {:>4}  {:16}  {:8}  {:>3}", "TID", "NAME", "STATE", "PRI"), ACCENT_CYAN));
    state.output.push((String::from("  ─────────────────────────────────────"), TEXT_MUTED));
    for (tid, name, thread_state, priority) in &threads {
        let state_str = format!("{:?}", thread_state);
        let color = match thread_state {
            crate::process::ThreadState::Running => ACCENT_GREEN,
            crate::process::ThreadState::Ready => TEXT_PRIMARY,
            _ => TEXT_MUTED,
        };
        state.output.push((format!("  {:>4}  {:16}  {:8}  {:>3}", tid, name, state_str, priority), color));
    }
    state.output.push((format!("  Total: {} threads", threads.len()), TEXT_SECONDARY));
}

fn cmd_mem(state: &mut TerminalState) {
    let (used, free) = crate::memory::heap::heap_stats();
    let total = used + free;
    let pct = if total > 0 { used * 100 / total } else { 0 };
    state.output.push((format!("  Heap: {}K used / {}K total ({}%)", used / 1024, total / 1024, pct), ACCENT_CYAN));

    // Simple ASCII bar
    let bar_width = 40;
    let filled = (pct as usize * bar_width) / 100;
    let bar: String = (0..bar_width).map(|i| if i < filled { '█' } else { '░' }).collect();
    let bar_color = if pct > 80 { ACCENT_RED } else if pct > 60 { ACCENT_ORANGE } else { ACCENT_GREEN };
    state.output.push((format!("  [{}]", bar), bar_color));
}

fn cmd_uptime(state: &mut TerminalState) {
    let secs = crate::drivers::timer::uptime_secs();
    let mins = secs / 60;
    let s = secs % 60;
    state.output.push((format!("  Uptime: {}m {}s ({} seconds total)", mins, s, secs), ACCENT_CYAN));
}

fn cmd_info(state: &mut TerminalState) {
    state.output.push((String::from("  Smart OS v0.4.0 — Hybrid Microkernel"), ACCENT_CYAN));
    state.output.push((String::from("  Architecture: x86_64"), TEXT_PRIMARY));
    state.output.push((format!("  SmartPack format v{}", smartpack::FORMAT_VERSION), TEXT_PRIMARY));

    let (used, free) = crate::memory::heap::heap_stats();
    state.output.push((format!("  Heap: {}K / {}K", used / 1024, (used + free) / 1024), TEXT_PRIMARY));

    let threads = crate::process::scheduler::ready_count() + 1;
    state.output.push((format!("  Threads: {}", threads), TEXT_PRIMARY));

    let uptime = crate::drivers::timer::uptime_secs();
    state.output.push((format!("  Uptime: {}s", uptime), TEXT_PRIMARY));

    let plugins = crate::plugins::registry::list_plugins();
    state.output.push((format!("  Plugins: {} loaded", plugins.len()), TEXT_PRIMARY));

    let (unique, total) = crate::smartfs::dedup_stats();
    state.output.push((format!("  SmartFS chunks: {}/{} (dedup)", unique, total), TEXT_PRIMARY));
}

fn cmd_classify(state: &mut TerminalState, file_arg: Option<&str>) {
    let file = match file_arg {
        Some(f) => f,
        None => {
            state.output.push((String::from("classify: missing file argument"), ACCENT_RED));
            return;
        }
    };
    let path = resolve_path(&state.cwd, file);
    match crate::smartfs::classify_file(&path) {
        Ok((category, confidence)) => {
            state.output.push((format!("  {} => {:?} ({:.0}% confidence)",
                path, category, confidence * 100.0), ACCENT_PURPLE));
        }
        Err(e) => {
            state.output.push((format!("classify: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_store(state: &mut TerminalState, file_arg: Option<&str>) {
    let file = match file_arg {
        Some(f) => f,
        None => {
            state.output.push((String::from("store: missing file argument"), ACCENT_RED));
            return;
        }
    };
    let path = resolve_path(&state.cwd, file);
    // Read the file content first
    match crate::vfs::open(&path) {
        Ok(fd) => {
            let mut buf = vec![0u8; 65536];
            match crate::vfs::read(fd, &mut buf) {
                Ok(n) => {
                    crate::vfs::close(fd).ok();
                    let data = &buf[..n];
                    match crate::smartfs::store_file(&path, data) {
                        Ok(()) => state.output.push((format!("  Stored {} via SmartFS ({} bytes)", path, n), ACCENT_GREEN)),
                        Err(e) => state.output.push((format!("store: {}", e), ACCENT_RED)),
                    }
                }
                Err(e) => {
                    crate::vfs::close(fd).ok();
                    state.output.push((format!("store: read error: {}", e), ACCENT_RED));
                }
            }
        }
        Err(e) => state.output.push((format!("store: {}", e), ACCENT_RED)),
    }
}

fn cmd_dedup(state: &mut TerminalState) {
    let (unique, total) = crate::smartfs::dedup_stats();
    state.output.push((format!("  SmartFS dedup: {} unique / {} total chunks", unique, total), ACCENT_ORANGE));
    if total > 0 {
        let saved = total.saturating_sub(unique);
        state.output.push((format!("  Savings: {} duplicate chunks avoided", saved), TEXT_PRIMARY));
    }
}

fn cmd_ports(state: &mut TerminalState) {
    let ports = crate::ipc::port::port_stats();
    state.output.push((format!("  {:20}  {:>8}", "PORT", "PENDING"), ACCENT_CYAN));
    state.output.push((String::from("  ─────────────────────────────"), TEXT_MUTED));
    for (name, count) in &ports {
        state.output.push((format!("  {:20}  {:>8}", name, count), TEXT_PRIMARY));
    }
    state.output.push((format!("  Total: {} ports", ports.len()), TEXT_SECONDARY));
}

fn cmd_plugins(state: &mut TerminalState) {
    let plugins = crate::plugins::registry::list_plugins();
    state.output.push((format!("  {:20}  {:>10}", "PLUGIN", "STATE"), ACCENT_CYAN));
    state.output.push((String::from("  ─────────────────────────────────"), TEXT_MUTED));
    for (name, plugin_state) in &plugins {
        let (state_str, color) = match plugin_state {
            crate::plugins::manifest::PluginState::Running => ("Running", ACCENT_GREEN),
            crate::plugins::manifest::PluginState::Stopped => ("Stopped", ACCENT_RED),
            crate::plugins::manifest::PluginState::Verified => ("Verified", ACCENT_ORANGE),
            _ => ("Other", TEXT_MUTED),
        };
        state.output.push((format!("  {:20}  {:>10}", name, state_str), color));
    }
}

// ═══════════════════════════════════════════════════════════════
//  Phase 6: Networking, Disk, and SMP commands
// ═══════════════════════════════════════════════════════════════

fn cmd_netinfo(state: &mut TerminalState) {
    state.output.push((String::from("  Network Interface:"), ACCENT_CYAN));
    if let Some(mac) = crate::drivers::virtio_net::mac_address() {
        state.output.push((format!(
            "  MAC:     {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ), TEXT_PRIMARY));
        state.output.push((format!(
            "  IP:      {}.{}.{}.{}",
            crate::net::LOCAL_IP[0], crate::net::LOCAL_IP[1],
            crate::net::LOCAL_IP[2], crate::net::LOCAL_IP[3],
        ), TEXT_PRIMARY));
        state.output.push((format!(
            "  Gateway: {}.{}.{}.{}",
            crate::net::GATEWAY_IP[0], crate::net::GATEWAY_IP[1],
            crate::net::GATEWAY_IP[2], crate::net::GATEWAY_IP[3],
        ), TEXT_PRIMARY));
        state.output.push((format!("  ARP cache: {} entries", crate::net::arp::cache_size()), TEXT_SECONDARY));
        state.output.push((format!("  UDP sockets: {}", crate::net::udp::socket_count()), TEXT_SECONDARY));
    } else {
        state.output.push((String::from("  No network device available."), ACCENT_RED));
    }
}

fn cmd_udpsend(state: &mut TerminalState, args: &[&str]) {
    if args.len() < 3 {
        state.output.push((String::from("  Usage: udpsend <ip> <port> <message>"), ACCENT_ORANGE));
        return;
    }
    let ip_parts: Vec<&str> = args[0].split('.').collect();
    if ip_parts.len() != 4 {
        state.output.push((String::from("  Invalid IP address format."), ACCENT_RED));
        return;
    }
    let mut ip = [0u8; 4];
    for i in 0..4 {
        ip[i] = match ip_parts[i].parse::<u8>() {
            Ok(v) => v,
            Err(_) => {
                state.output.push((String::from("  Invalid IP octet."), ACCENT_RED));
                return;
            }
        };
    }
    let port: u16 = match args[1].parse() {
        Ok(p) => p,
        Err(_) => {
            state.output.push((String::from("  Invalid port number."), ACCENT_RED));
            return;
        }
    };
    let message: String = args[2..].join(" ");
    match crate::net::udp::send(ip, port, 12345, message.as_bytes()) {
        Ok(()) => state.output.push((format!(
            "  Sent {} bytes to {}.{}.{}.{}:{}",
            message.len(), ip[0], ip[1], ip[2], ip[3], port
        ), ACCENT_GREEN)),
        Err(e) => state.output.push((format!("  Send failed: {}", e), ACCENT_RED)),
    }
}

fn cmd_diskinfo(state: &mut TerminalState) {
    state.output.push((String::from("  Disk Device:"), ACCENT_CYAN));
    if crate::drivers::virtio_blk::is_available() {
        let cap = crate::drivers::virtio_blk::capacity();
        state.output.push((format!("  Capacity: {} sectors ({} KiB)", cap, cap * 512 / 1024), TEXT_PRIMARY));
        if crate::drivers::diskfs::is_available() {
            state.output.push((String::from("  Filesystem: mounted (SmartFS-on-disk)"), ACCENT_GREEN));
            let diskfs = crate::drivers::diskfs::DISK_FS.lock();
            if let Some(fs) = diskfs.as_ref() {
                let file_count = fs.list_files().len();
                state.output.push((format!("  Files: {}", file_count), TEXT_PRIMARY));
            }
        } else {
            state.output.push((String::from("  Filesystem: not mounted"), ACCENT_ORANGE));
        }
    } else {
        state.output.push((String::from("  No block device available."), ACCENT_RED));
    }
}

fn cmd_diskls(state: &mut TerminalState) {
    let diskfs = crate::drivers::diskfs::DISK_FS.lock();
    match diskfs.as_ref() {
        Some(fs) => {
            let files = fs.list_files();
            if files.is_empty() {
                state.output.push((String::from("  (empty disk)"), TEXT_MUTED));
            } else {
                state.output.push((format!("  {:30}  {:>8}", "NAME", "SIZE"), ACCENT_CYAN));
                state.output.push((String::from("  ──────────────────────────────────────"), TEXT_MUTED));
                for name in &files {
                    let size = fs.read_file(name).map(|d| d.len()).unwrap_or(0);
                    state.output.push((format!("  {:30}  {:>8}", name, size), TEXT_PRIMARY));
                }
                state.output.push((format!("  {} file(s)", files.len()), TEXT_SECONDARY));
            }
        }
        None => {
            state.output.push((String::from("  Disk not mounted."), ACCENT_RED));
        }
    }
}

fn cmd_diskwrite(state: &mut TerminalState, args: &[&str]) {
    if args.len() < 2 {
        state.output.push((String::from("  Usage: diskwrite <filename> <content>"), ACCENT_ORANGE));
        return;
    }
    let name = args[0];
    let content: String = args[1..].join(" ");
    let mut diskfs = crate::drivers::diskfs::DISK_FS.lock();
    match diskfs.as_mut() {
        Some(fs) => {
            match fs.write_file(name, content.as_bytes()) {
                Ok(()) => state.output.push((format!("  Written {} bytes to /disk/{}", content.len(), name), ACCENT_GREEN)),
                Err(e) => state.output.push((format!("  Write failed: {}", e), ACCENT_RED)),
            }
        }
        None => state.output.push((String::from("  Disk not mounted."), ACCENT_RED)),
    }
}

fn cmd_diskcat(state: &mut TerminalState, file: Option<&str>) {
    let name = match file {
        Some(f) => f,
        None => {
            state.output.push((String::from("  Usage: diskcat <filename>"), ACCENT_ORANGE));
            return;
        }
    };
    let diskfs = crate::drivers::diskfs::DISK_FS.lock();
    match diskfs.as_ref() {
        Some(fs) => {
            match fs.read_file(name) {
                Ok(data) => {
                    if let Ok(text) = core::str::from_utf8(&data) {
                        for line in text.lines() {
                            state.output.push((format!("  {}", line), TEXT_PRIMARY));
                        }
                    } else {
                        state.output.push((format!("  (binary data, {} bytes)", data.len()), TEXT_MUTED));
                    }
                }
                Err(e) => state.output.push((format!("  Read failed: {}", e), ACCENT_RED)),
            }
        }
        None => state.output.push((String::from("  Disk not mounted."), ACCENT_RED)),
    }
}

fn cmd_cpus(state: &mut TerminalState) {
    let count = crate::arch::x86_64::smp::cpu_count();
    let bsp_id = crate::arch::x86_64::lapic::lapic_id();
    state.output.push((String::from("  CPU Information:"), ACCENT_CYAN));
    state.output.push((format!("  Active CPUs: {}", count), TEXT_PRIMARY));
    state.output.push((format!("  BSP LAPIC ID: {}", bsp_id), TEXT_PRIMARY));
    if crate::arch::x86_64::lapic::is_active() {
        state.output.push((String::from("  LAPIC: active (periodic timer)"), ACCENT_GREEN));
    } else {
        state.output.push((String::from("  LAPIC: inactive (using PIT)"), ACCENT_ORANGE));
    }
}

// ── Phase 7: Intelligent OS Commands ────────────────────────────────

fn cmd_predict(state: &mut TerminalState) {
    state.output.push((String::from("  Predictive Scheduling:"), ACCENT_CYAN));
    state.output.push((format!("  NPU: {}", crate::ai::npu::npu_name()), TEXT_PRIMARY));

    let predictions = crate::ai::predictor::predict_next(5);
    if predictions.is_empty() {
        state.output.push((String::from("  No predictions yet (need more usage data)."), TEXT_SECONDARY));
    } else {
        state.output.push((String::from("  Top predicted next apps:"), TEXT_PRIMARY));
        for (name, confidence) in &predictions {
            let bar_len = (*confidence * 20.0) as usize;
            let bar: String = core::iter::repeat('█').take(bar_len).collect();
            state.output.push((
                format!("    {:12} {:5.1}% {}", name, confidence * 100.0, bar),
                ACCENT_GREEN,
            ));
        }
    }

    let warm = crate::ai::prefetch::warm_count();
    state.output.push((format!("  Warm cache: {} apps pre-loaded", warm), TEXT_PRIMARY));
}

fn cmd_kginfo(state: &mut TerminalState) {
    state.output.push((String::from("  Knowledge Graph:"), ACCENT_CYAN));

    let g = crate::knowledge::graph::GRAPH.lock();
    match g.as_ref() {
        Some(graph) => {
            state.output.push((format!("  Nodes: {}", graph.node_count()), TEXT_PRIMARY));
            state.output.push((format!("  Edges: {}", graph.edge_count()), TEXT_PRIMARY));
            let types = graph.node_types();
            if !types.is_empty() {
                state.output.push((String::from("  Node types:"), TEXT_PRIMARY));
                for t in &types {
                    let count = graph.find_by_type(t).len();
                    state.output.push((format!("    {} ({})", t, count), ACCENT_GREEN));
                }
            }
        }
        None => {
            state.output.push((String::from("  Knowledge graph not initialized."), ACCENT_RED));
        }
    }
}

fn cmd_kgquery(state: &mut TerminalState, node_type: Option<&str>) {
    let type_str = match node_type {
        Some(t) => t,
        None => {
            state.output.push((String::from("  Usage: kgquery <type>"), ACCENT_ORANGE));
            return;
        }
    };

    let g = crate::knowledge::graph::GRAPH.lock();
    match g.as_ref() {
        Some(graph) => {
            let nodes = graph.find_by_type(type_str);
            if nodes.is_empty() {
                state.output.push((format!("  No nodes of type '{}'.", type_str), TEXT_SECONDARY));
            } else {
                state.output.push((format!("  Nodes of type '{}' ({}):", type_str, nodes.len()), ACCENT_CYAN));
                for node in &nodes {
                    state.output.push((format!("    [{}] {}", node.id, format_value_brief(&node.properties)), TEXT_PRIMARY));
                }
            }
        }
        None => {
            state.output.push((String::from("  Knowledge graph not initialized."), ACCENT_RED));
        }
    }
}

fn cmd_security(state: &mut TerminalState) {
    state.output.push((String::from("  Security Anomaly Detection:"), ACCENT_CYAN));
    let (total, flagged) = crate::security::monitor::monitor_stats();
    state.output.push((format!("  Monitored processes: {}", total), TEXT_PRIMARY));

    if flagged > 0 {
        state.output.push((format!("  FLAGGED processes: {}", flagged), ACCENT_RED));
    } else {
        state.output.push((String::from("  No anomalies detected."), ACCENT_GREEN));
    }

    let details = crate::security::monitor::monitor_details();
    if !details.is_empty() {
        state.output.push((String::from("  Process details:"), TEXT_PRIMARY));
        for (pid, calls, score, flagged) in &details {
            let status = if *flagged { "FROZEN" } else { "ok" };
            state.output.push((
                format!("    pid={} calls={} score={:.2} [{}]", pid, calls, score, status),
                if *flagged { ACCENT_RED } else { TEXT_SECONDARY },
            ));
        }
    }
}

fn cmd_partinfo(state: &mut TerminalState) {
    state.output.push((String::from("  A/B System Partitions:"), ACCENT_CYAN));

    let mgr = crate::immutable::partition::PARTITIONS.lock();
    match mgr.as_ref() {
        Some(m) => {
            let active_label = if m.active_idx == 0 { "A" } else { "B" };
            state.output.push((format!("  Active: Partition {}", active_label), ACCENT_GREEN));
            for p in &m.partitions {
                let label = if p.id == 0 { "A" } else { "B" };
                let marker = if p.id as usize == m.active_idx { " ◄" } else { "" };
                state.output.push((
                    format!("    [{}] v{} | {} | boots: {}{}",
                        label, p.version, p.state.as_str(), p.boot_count, marker),
                    TEXT_PRIMARY,
                ));
            }

            // Show protected paths
            let paths = crate::immutable::protection::protected_paths();
            state.output.push((String::from("  Protected paths:"), TEXT_PRIMARY));
            for path in &paths {
                state.output.push((format!("    {}", path), ACCENT_ORANGE));
            }
        }
        None => {
            state.output.push((String::from("  Partition manager not initialized."), ACCENT_RED));
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  Phase 8: TCP Commands
// ═══════════════════════════════════════════════════════════════

fn cmd_tcpinfo(state: &mut TerminalState) {
    use crate::net::tcp;
    state.output.push((String::from("── TCP Connections ──"), ACCENT_CYAN));
    let conns = tcp::connection_list();
    if conns.is_empty() {
        state.output.push((String::from("  No active connections."), TEXT_SECONDARY));
    } else {
        state.output.push((format!("  {} active connection(s):", conns.len()), TEXT_PRIMARY));
        for (id, tcp_state, local_port, remote_ip, remote_port) in &conns {
            state.output.push((format!(
                "  [{}] :{} → {}.{}.{}.{}:{} ({})",
                id, local_port,
                remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3],
                remote_port,
                tcp_state_name(tcp_state),
            ), ACCENT_GREEN));
        }
    }
    let listeners = tcp::listener_count();
    state.output.push((format!("  {} listener(s) active.", listeners), TEXT_SECONDARY));
}

fn tcp_state_name(state: &crate::net::tcp::TcpState) -> &'static str {
    use crate::net::tcp::TcpState;
    match state {
        TcpState::Closed => "CLOSED",
        TcpState::Listen => "LISTEN",
        TcpState::SynSent => "SYN_SENT",
        TcpState::SynReceived => "SYN_RCVD",
        TcpState::Established => "ESTABLISHED",
        TcpState::FinWait1 => "FIN_WAIT_1",
        TcpState::FinWait2 => "FIN_WAIT_2",
        TcpState::CloseWait => "CLOSE_WAIT",
        TcpState::LastAck => "LAST_ACK",
        TcpState::TimeWait => "TIME_WAIT",
    }
}

fn cmd_tcpconnect(state: &mut TerminalState, args: &[&str]) {
    if args.len() < 2 {
        state.output.push((String::from("Usage: tcpconnect <ip> <port>"), ACCENT_ORANGE));
        return;
    }
    let ip = parse_ip(args[0]);
    let port: u16 = match args[1].parse() {
        Ok(p) => p,
        Err(_) => {
            state.output.push((String::from("Invalid port number."), ACCENT_RED));
            return;
        }
    };
    let local_port = crate::net::tcp::alloc_ephemeral_port();
    match crate::net::tcp::connect(ip, port, local_port) {
        Ok(sock) => {
            state.output.push((format!("TCP connection initiated, socket id={}.", sock), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("TCP connect failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_tcpsend(state: &mut TerminalState, args: &[&str]) {
    if args.len() < 2 {
        state.output.push((String::from("Usage: tcpsend <sock_id> <message>"), ACCENT_ORANGE));
        return;
    }
    let sock_id: u64 = match args[0].parse() {
        Ok(id) => id,
        Err(_) => {
            state.output.push((String::from("Invalid socket id."), ACCENT_RED));
            return;
        }
    };
    let msg = args[1..].join(" ");
    match crate::net::tcp::send(sock_id, msg.as_bytes()) {
        Ok(n) => {
            state.output.push((format!("Sent {} bytes on socket {}.", n, sock_id), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("TCP send failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_tcprecv(state: &mut TerminalState, args: &[&str]) {
    if args.is_empty() {
        state.output.push((String::from("Usage: tcprecv <sock_id>"), ACCENT_ORANGE));
        return;
    }
    let sock_id: u64 = match args[0].parse() {
        Ok(id) => id,
        Err(_) => {
            state.output.push((String::from("Invalid socket id."), ACCENT_RED));
            return;
        }
    };
    let mut buf = [0u8; 1024];
    match crate::net::tcp::recv(sock_id, &mut buf) {
        Ok(n) if n > 0 => {
            let text = core::str::from_utf8(&buf[..n]).unwrap_or("<binary data>");
            state.output.push((format!("Received {} bytes: {}", n, text), ACCENT_GREEN));
        }
        Ok(_) => {
            state.output.push((String::from("No data available."), TEXT_SECONDARY));
        }
        Err(e) => {
            state.output.push((format!("TCP recv failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_tcpclose(state: &mut TerminalState, arg: Option<&str>) {
    let sock_id: u64 = match arg.and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => {
            state.output.push((String::from("Usage: tcpclose <sock_id>"), ACCENT_ORANGE));
            return;
        }
    };
    match crate::net::tcp::close(sock_id) {
        Ok(()) => {
            state.output.push((format!("TCP socket {} close initiated.", sock_id), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("TCP close failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_tcplisten(state: &mut TerminalState, arg: Option<&str>) {
    let port: u16 = match arg.and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => {
            state.output.push((String::from("Usage: tcplisten <port>"), ACCENT_ORANGE));
            return;
        }
    };
    match crate::net::tcp::listen(port) {
        Ok(()) => {
            state.output.push((format!("Listening on TCP port {}.", port), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("TCP listen failed: {}", e), ACCENT_RED));
        }
    }
}

fn parse_ip(s: &str) -> [u8; 4] {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() == 4 {
        let a = parts[0].parse().unwrap_or(0);
        let b = parts[1].parse().unwrap_or(0);
        let c = parts[2].parse().unwrap_or(0);
        let d = parts[3].parse().unwrap_or(0);
        [a, b, c, d]
    } else {
        [0, 0, 0, 0]
    }
}

// ═══════════════════════════════════════════════════════════════
//  Phase 8: USB / FAT32 / Editor Commands
// ═══════════════════════════════════════════════════════════════

fn cmd_usbinfo(state: &mut TerminalState) {
    use crate::drivers::xhci;
    state.output.push((String::from("── USB Devices (xHCI) ──"), ACCENT_CYAN));
    if !xhci::is_available() {
        state.output.push((String::from("  xHCI controller not initialized."), TEXT_SECONDARY));
        return;
    }
    let devices = xhci::device_list();
    if devices.is_empty() {
        state.output.push((String::from("  No USB devices found."), TEXT_SECONDARY));
    } else {
        state.output.push((format!("  {} device(s):", devices.len()), TEXT_PRIMARY));
        for dev in &devices {
            state.output.push((format!(
                "  Slot {} Port {} Class={}:{} Proto={} {:04x}:{:04x}",
                dev.slot_id, dev.port, dev.class, dev.subclass,
                dev.protocol, dev.vendor_id, dev.product_id,
            ), ACCENT_GREEN));
        }
    }
}

fn cmd_fat32ls(state: &mut TerminalState, arg: Option<&str>) {
    use crate::drivers::fat32;
    if !fat32::is_available() {
        state.output.push((String::from("FAT32 not mounted."), ACCENT_RED));
        return;
    }
    let path = arg.unwrap_or("/");
    match fat32::list_dir(path) {
        Ok(entries) => {
            state.output.push((format!("FAT32 directory: {}", path), ACCENT_CYAN));
            if entries.is_empty() {
                state.output.push((String::from("  (empty)"), TEXT_SECONDARY));
            } else {
                for entry in &entries {
                    state.output.push((format!("  {}", entry), TEXT_PRIMARY));
                }
            }
        }
        Err(e) => {
            state.output.push((format!("fat32ls error: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_fat32cat(state: &mut TerminalState, arg: Option<&str>) {
    use crate::drivers::fat32;
    let path = match arg {
        Some(p) => p,
        None => {
            state.output.push((String::from("Usage: fat32cat <file>"), ACCENT_ORANGE));
            return;
        }
    };
    if !fat32::is_available() {
        state.output.push((String::from("FAT32 not mounted."), ACCENT_RED));
        return;
    }
    match fat32::read_file(path) {
        Ok(data) => {
            let text = core::str::from_utf8(&data).unwrap_or("<binary data>");
            for line in text.lines().take(50) {
                state.output.push((String::from(line), TEXT_PRIMARY));
            }
        }
        Err(e) => {
            state.output.push((format!("fat32cat error: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_edit(state: &mut TerminalState, arg: Option<&str>) {
    if let Some(path) = arg {
        let full = resolve_path(&state.cwd, path);
        crate::apps::editor::set_initial_file(&full);
    }
    // Spawn the editor as a new thread (if not already running)
    crate::process::scheduler::spawn("editor", crate::apps::editor::run, 11);
    state.output.push((String::from("Text editor launched."), ACCENT_GREEN));
}

/// Format a SmartPack Value briefly for display.
fn format_value_brief(value: &smartpack::Value) -> String {
    match value {
        smartpack::Value::Map(entries) => {
            let pairs: Vec<String> = entries
                .iter()
                .take(3)
                .filter_map(|(k, v)| {
                    if let smartpack::Value::String(ks) = k {
                        Some(format!("{}={}", ks, format_value_leaf(v)))
                    } else {
                        None
                    }
                })
                .collect();
            let more = if entries.len() > 3 {
                format!(", +{}", entries.len() - 3)
            } else {
                String::new()
            };
            format!("{{{}{}}}", pairs.join(", "), more)
        }
        other => format_value_leaf(other),
    }
}

/// Format a leaf SmartPack Value for display.
fn format_value_leaf(value: &smartpack::Value) -> String {
    match value {
        smartpack::Value::String(s) => format!("\"{}\"", s),
        smartpack::Value::UInt32(n) => format!("{}", n),
        smartpack::Value::UInt64(n) => format!("{}", n),
        smartpack::Value::Null => String::from("null"),
        smartpack::Value::Bool(b) => format!("{}", b),
        _ => String::from("..."),
    }
}

// ═══════════════════════════════════════════════════════════════
//  Phase 9: Desktop OS Polish Commands
// ═══════════════════════════════════════════════════════════════

fn cmd_date(state: &mut TerminalState) {
    let dt = crate::drivers::rtc::now();
    state.output.push((format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    ), ACCENT_GREEN));
}

fn cmd_hostname(state: &mut TerminalState) {
    let name = crate::process::env::get("HOSTNAME")
        .unwrap_or(String::from("smartos"));
    state.output.push((name, TEXT_PRIMARY));
}

fn cmd_whoami(state: &mut TerminalState) {
    let user = crate::process::env::get("USER")
        .unwrap_or(String::from("root"));
    state.output.push((user, TEXT_PRIMARY));
}

fn cmd_env(state: &mut TerminalState) {
    let vars = crate::process::env::list();
    if vars.is_empty() {
        state.output.push((String::from("No environment variables set."), TEXT_MUTED));
        return;
    }
    state.output.push((String::from("Environment variables:"), ACCENT_CYAN));
    for (key, value) in vars {
        state.output.push((format!("  {}={}", key, value), TEXT_PRIMARY));
    }
}

fn cmd_export(state: &mut TerminalState, args: &[&str]) {
    if args.is_empty() {
        state.output.push((String::from("Usage: export KEY=VALUE"), ACCENT_RED));
        return;
    }
    let arg = args.join(" ");
    if let Some(eq_pos) = arg.find('=') {
        let key = &arg[..eq_pos];
        let val = &arg[eq_pos + 1..];
        crate::process::env::set(key, val);
        state.output.push((format!("{}={}", key, val), ACCENT_GREEN));
    } else {
        state.output.push((String::from("Usage: export KEY=VALUE"), ACCENT_RED));
    }
}

fn cmd_kill(state: &mut TerminalState, arg: Option<&str>) {
    let tid_str = match arg {
        Some(s) => s,
        None => {
            state.output.push((String::from("Usage: kill <tid>"), ACCENT_RED));
            return;
        }
    };
    if let Ok(tid) = tid_str.parse::<u64>() {
        // Don't allow killing boot thread (tid 0) or gui renderer
        if tid <= 2 {
            state.output.push((format!("Cannot kill protected thread {}", tid), ACCENT_RED));
            return;
        }
        match crate::process::signal::send_signal(tid, crate::process::signal::Signal::Kill) {
            Ok(()) => state.output.push((format!("Signal KILL sent to thread {}", tid), ACCENT_ORANGE)),
            Err(e) => state.output.push((format!("Failed: {}", e), ACCENT_RED)),
        }
    } else {
        state.output.push((String::from("Invalid TID. Usage: kill <number>"), ACCENT_RED));
    }
}

fn cmd_dns(state: &mut TerminalState, arg: Option<&str>) {
    let hostname = match arg {
        Some(h) => h,
        None => {
            state.output.push((String::from("Usage: dns <hostname>"), ACCENT_RED));
            return;
        }
    };
    state.output.push((format!("Resolving {}...", hostname), TEXT_MUTED));
    match crate::net::dns::resolve(hostname) {
        Ok(ip) => {
            state.output.push((format!("{} => {}.{}.{}.{}", hostname, ip[0], ip[1], ip[2], ip[3]), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("DNS error: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_calc(state: &mut TerminalState) {
    state.output.push((String::from("Calculator is running (check taskbar)."), ACCENT_MAGENTA));
}

fn cmd_taskmgr(state: &mut TerminalState) {
    state.output.push((String::from("Task Manager is running (check taskbar)."), ACCENT_ORANGE));
}

fn cmd_settings(state: &mut TerminalState) {
    state.output.push((String::from("Settings is running (check taskbar)."), ACCENT_PURPLE));
}

fn cmd_notify(state: &mut TerminalState, args: &[&str]) {
    if args.is_empty() {
        state.output.push((String::from("Usage: notify <message>"), ACCENT_RED));
        return;
    }
    let msg = args.join(" ");
    crate::gui::notification::push("Terminal", &msg, ACCENT_GREEN);
    state.output.push((format!("Notification sent: {}", msg), TEXT_MUTED));
}

fn cmd_clipboard(state: &mut TerminalState) {
    match crate::gui::clipboard::paste() {
        Some(text) => {
            state.output.push((String::from("Clipboard contents:"), ACCENT_CYAN));
            state.output.push((text, TEXT_PRIMARY));
        }
        None => {
            state.output.push((String::from("Clipboard is empty."), TEXT_MUTED));
        }
    }
}

fn cmd_version(state: &mut TerminalState) {
    state.output.push((String::from("Smart OS v0.11.0"), ACCENT_CYAN));
    state.output.push((String::from("Phase 11: Userland & Real Programs"), TEXT_PRIMARY));
    state.output.push((format!("Uptime: {}s", crate::drivers::timer::uptime_secs()), TEXT_SECONDARY));
    let dt = crate::drivers::rtc::now();
    state.output.push((format!("Date: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second), TEXT_SECONDARY));
}

// ═══════════════════════════════════════════════════════════════
//  Phase 10: Kernel Maturity Commands
// ═══════════════════════════════════════════════════════════════

fn cmd_cowinfo(state: &mut TerminalState) {
    let (shared, total_refcounted) = crate::memory::cow::cow_stats();
    state.output.push((String::from("--- Copy-on-Write Stats ---"), ACCENT_CYAN));
    state.output.push((format!("  Shared frames (refcount>1): {}", shared), TEXT_PRIMARY));
    state.output.push((format!("  Total refcounted frames:    {}", total_refcounted), TEXT_PRIMARY));
}

fn cmd_shmem(state: &mut TerminalState, args: &[&str]) {
    if args.is_empty() {
        // Show shared memory stats
        let (count, total_pages) = crate::memory::shmem::shmem_stats();
        state.output.push((String::from("--- Shared Memory ---"), ACCENT_CYAN));
        state.output.push((format!("  Segments: {}", count), TEXT_PRIMARY));
        state.output.push((format!("  Total pages: {} ({} KiB)", total_pages, total_pages * 4), TEXT_PRIMARY));
        let segs = crate::memory::shmem::list_segments();
        if segs.is_empty() {
            state.output.push((String::from("  (no segments)"), TEXT_MUTED));
        } else {
            for (id, name, pages, attached) in segs {
                state.output.push((format!("  [{}] '{}': {} pages, {} attached", id, name, pages, attached), TEXT_SECONDARY));
            }
        }
    } else if args.len() >= 2 {
        // Create: shmem <name> <pages>
        let name = args[0];
        if let Ok(pages) = args[1].parse::<usize>() {
            match crate::memory::shmem::create(name, pages, 0) {
                Some(id) => state.output.push((format!("Created shmem '{}' ({} pages) id={}", name, pages, id), ACCENT_GREEN)),
                None => state.output.push((String::from("Failed to create segment"), ACCENT_RED)),
            }
        } else {
            state.output.push((String::from("Usage: shmem <name> <pages>"), ACCENT_RED));
        }
    } else {
        state.output.push((String::from("Usage: shmem | shmem <name> <pages>"), ACCENT_RED));
    }
}

fn cmd_aslr(state: &mut TerminalState) {
    state.output.push((String::from("--- ASLR Info ---"), ACCENT_CYAN));
    state.output.push((format!("  Enabled: {}", crate::memory::aslr::is_enabled()), TEXT_PRIMARY));
    // Show sample randomized addresses
    let stack = crate::memory::aslr::randomize_stack_top();
    let elf = crate::memory::aslr::randomize_elf_base();
    let mmap = crate::memory::aslr::randomize_mmap_base();
    let heap = crate::memory::aslr::randomize_heap_start(elf + 0x10_0000);
    state.output.push((format!("  Sample stack top:  {:#x}", stack), TEXT_SECONDARY));
    state.output.push((format!("  Sample ELF base:   {:#x}", elf), TEXT_SECONDARY));
    state.output.push((format!("  Sample mmap base:  {:#x}", mmap), TEXT_SECONDARY));
    state.output.push((format!("  Sample heap start: {:#x}", heap), TEXT_SECONDARY));
}

fn cmd_swapinfo(state: &mut TerminalState) {
    let (page_faults, cow_faults, swap_faults, swap_outs, swap_ins, available) = crate::memory::swap::swap_stats();
    state.output.push((String::from("--- Page Fault / Swap Stats ---"), ACCENT_CYAN));
    state.output.push((format!("  Swap available: {}", available), TEXT_PRIMARY));
    state.output.push((format!("  Page faults:    {}", page_faults), TEXT_PRIMARY));
    state.output.push((format!("  CoW faults:     {}", cow_faults), TEXT_PRIMARY));
    state.output.push((format!("  Swap faults:    {}", swap_faults), TEXT_PRIMARY));
    state.output.push((format!("  Swap-outs:      {}", swap_outs), TEXT_PRIMARY));
    state.output.push((format!("  Swap-ins:       {}", swap_ins), TEXT_PRIMARY));
    let swapped = crate::memory::swap::swapped_page_count();
    state.output.push((format!("  Pages swapped:  {} ({} KiB)", swapped, swapped * 4), TEXT_SECONDARY));
}

fn cmd_smpinfo(state: &mut TerminalState) {
    state.output.push((String::from("--- SMP Load Balancing ---"), ACCENT_CYAN));
    state.output.push((format!("  Active CPUs: {}", crate::process::smp_balance::active_cpu_count()), TEXT_PRIMARY));
    state.output.push((format!("  Steal ops:   {}", crate::process::smp_balance::steal_count()), TEXT_PRIMARY));
    let loads = crate::process::smp_balance::cpu_load_stats();
    for (cpu_id, queue_len, tasks_done, active) in &loads {
        let status = if *active { "active" } else { "idle" };
        let bar_len = (*queue_len).min(20);
        let bar: String = (0..bar_len).map(|_| '█').collect();
        state.output.push((format!("  CPU {}: {:2} queued, {} done [{}] {}", cpu_id, queue_len, tasks_done, status, bar), TEXT_PRIMARY));
    }
}

fn cmd_strace(state: &mut TerminalState, arg: Option<&str>) {
    match arg {
        Some(tid_str) => {
            if let Ok(tid) = tid_str.parse::<u64>() {
                let events = crate::process::strace::get_trace(tid);
                if events.is_empty() {
                    state.output.push((format!("No trace data for tid {}", tid), TEXT_MUTED));
                    state.output.push((String::from("Enable with: strace (traces are auto-enabled globally)"), TEXT_MUTED));
                } else {
                    state.output.push((format!("--- Trace for tid {} ({} events) ---", tid, events.len()), ACCENT_CYAN));
                    for ev in events.iter().rev().take(20) {
                        state.output.push((crate::process::strace::format_event(ev), TEXT_SECONDARY));
                    }
                }
            } else {
                state.output.push((String::from("Usage: strace <tid>"), ACCENT_RED));
            }
        }
        None => {
            state.output.push((String::from("--- Syscall Tracing ---"), ACCENT_CYAN));
            state.output.push((format!("  Global tracing: {}", if crate::process::strace::is_global_enabled() { "ON" } else { "OFF" }), TEXT_PRIMARY));
            state.output.push((String::from("Usage: strace <tid> — show recent syscalls for thread"), TEXT_MUTED));
        }
    }
}

fn cmd_sandbox(state: &mut TerminalState) {
    state.output.push((String::from("--- Capability Sandboxes ---"), ACCENT_CYAN));
    state.output.push((format!("  Total sandboxed: {}", crate::security::sandbox::sandbox_count()), TEXT_PRIMARY));
    let sandboxes = crate::security::sandbox::list_sandboxes();
    if sandboxes.is_empty() {
        state.output.push((String::from("  (no active sandboxes)"), TEXT_MUTED));
    } else {
        for (pid, name, caps, denies) in sandboxes {
            state.output.push((format!("  PID {} '{}': {} [denied:{}]", pid, name, crate::security::sandbox::format_caps(caps), denies), TEXT_PRIMARY));
        }
    }
}

fn cmd_gdbinfo(state: &mut TerminalState) {
    let (active, connected, bp_count) = crate::drivers::gdb_stub::gdb_stats();
    state.output.push((String::from("--- GDB Debug Stub ---"), ACCENT_CYAN));
    state.output.push((format!("  Active:      {}", active), TEXT_PRIMARY));
    state.output.push((format!("  Connected:   {}", connected), TEXT_PRIMARY));
    state.output.push((format!("  Breakpoints: {}", bp_count), TEXT_PRIMARY));
    state.output.push((String::from("  Serial port: COM1 (0x3F8)"), TEXT_SECONDARY));
    state.output.push((String::from("  Protocol:    GDB Remote Serial"), TEXT_SECONDARY));
}

fn cmd_workspace(state: &mut TerminalState, arg: Option<&str>) {
    match arg {
        Some(n_str) => {
            if let Ok(n) = n_str.parse::<usize>() {
                if n < crate::gui::virtual_desktop::MAX_WORKSPACES {
                    crate::gui::virtual_desktop::switch_to(n);
                    let name = crate::gui::virtual_desktop::workspace_name(n);
                    state.output.push((format!("Switched to workspace {}: '{}'", n, name), ACCENT_GREEN));
                } else {
                    state.output.push((format!("Invalid workspace. Use 0-{}", crate::gui::virtual_desktop::MAX_WORKSPACES - 1), ACCENT_RED));
                }
            } else {
                state.output.push((String::from("Usage: workspace <0-3>"), ACCENT_RED));
            }
        }
        None => {
            state.output.push((String::from("--- Virtual Desktops ---"), ACCENT_CYAN));
            let info = crate::gui::virtual_desktop::workspace_info();
            for (idx, name, count, is_cur) in info {
                let marker = if is_cur { " ◄ active" } else { "" };
                state.output.push((format!("  [{}] '{}': {} windows{}", idx, name, count, marker), TEXT_PRIMARY));
            }
        }
    }
}

fn cmd_theme(state: &mut TerminalState, arg: Option<&str>) {
    match arg {
        Some(name) => {
            // Try to match theme by name (case-insensitive)
            let themes = crate::gui::theming::list_themes();
            let found = themes.iter().find(|(_, n)| n.eq_ignore_ascii_case(name));
            match found {
                Some((idx, theme_name)) => {
                    crate::gui::theming::set_theme(*idx);
                    state.output.push((format!("Theme changed to '{}'", theme_name), ACCENT_GREEN));
                }
                None => {
                    // Try as index
                    if let Ok(idx) = name.parse::<usize>() {
                        if idx < crate::gui::theming::theme_count() {
                            crate::gui::theming::set_theme(idx);
                            state.output.push((format!("Theme changed to '{}'", crate::gui::theming::current_theme_name()), ACCENT_GREEN));
                        } else {
                            state.output.push((format!("Invalid theme index. Use 0-{}", crate::gui::theming::theme_count() - 1), ACCENT_RED));
                        }
                    } else {
                        state.output.push((format!("Unknown theme '{}'. Available:", name), ACCENT_RED));
                        for (i, n) in crate::gui::theming::list_themes() {
                            state.output.push((format!("  {}: {}", i, n), TEXT_SECONDARY));
                        }
                    }
                }
            }
        }
        None => {
            state.output.push((String::from("--- Color Themes ---"), ACCENT_CYAN));
            state.output.push((format!("  Active: '{}'", crate::gui::theming::current_theme_name()), TEXT_PRIMARY));
            for (i, n) in crate::gui::theming::list_themes() {
                let marker = if n == crate::gui::theming::current_theme_name() { " ◄" } else { "" };
                state.output.push((format!("  {}: {}{}", i, n, marker), TEXT_SECONDARY));
            }
            state.output.push((String::from("Usage: theme <name|index>"), TEXT_MUTED));
        }
    }
}

fn cmd_ipv6info(state: &mut TerminalState) {
    let (enabled, addr) = crate::net::ipv6::ipv6_stats();
    state.output.push((String::from("--- IPv6 Network ---"), ACCENT_CYAN));
    state.output.push((format!("  Enabled:    {}", enabled), TEXT_PRIMARY));
    state.output.push((format!("  Link-local: {}", addr), TEXT_PRIMARY));
}

// ═══════════════════════════════════════════════════════════════
//  Phase 11: Userland & Real Programs Commands
// ═══════════════════════════════════════════════════════════════

fn cmd_exec(state: &mut TerminalState, path: Option<&str>) {
    let path = match path {
        Some(p) => p,
        None => {
            state.output.push((String::from("Usage: exec <path>"), ACCENT_RED));
            return;
        }
    };
    state.output.push((format!("Spawning user process from '{}'...", path), TEXT_PRIMARY));
    match crate::process::scheduler::spawn_user_process(path.rsplit('/').next().unwrap_or(path), path) {
        Ok(pid) => {
            state.output.push((format!("  User process spawned (pid={})", pid), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("  Failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_procs(state: &mut TerminalState) {
    let procs = crate::process::process::process_list();
    state.output.push((String::from("--- Process Table ---"), ACCENT_CYAN));
    state.output.push((format!("  {:>4} {:>4} {:12} {:8} {:>4} {}", "PID", "PPID", "NAME", "STATE", "EXIT", "CHILDREN"), TEXT_SECONDARY));
    for (pid, name, pstate, parent, exit_code, children) in procs {
        let state_str = match pstate {
            crate::process::process::ProcessState::Running => "running",
            crate::process::process::ProcessState::Zombie => "zombie",
        };
        let exit_str = match exit_code {
            Some(c) => format!("{}", c),
            None => String::from("-"),
        };
        let children_str = if children.is_empty() {
            String::from("-")
        } else {
            children.iter().map(|c| format!("{}", c)).collect::<Vec<_>>().join(",")
        };
        state.output.push((format!("  {:>4} {:>4} {:12} {:8} {:>4} {}", pid, parent, name, state_str, exit_str, children_str), TEXT_PRIMARY));
    }
}

fn cmd_usrrun(state: &mut TerminalState, name: Option<&str>) {
    let name = match name {
        Some(n) => n,
        None => {
            state.output.push((String::from("Usage: usrrun <name>  (runs /bin/<name>)"), ACCENT_RED));
            state.output.push((String::from("Available: true, false, echo, cat, ls, sh, hello, forktest"), TEXT_SECONDARY));
            return;
        }
    };
    let path = format!("/bin/{}", name);
    state.output.push((format!("Running /bin/{}...", name), TEXT_PRIMARY));
    match crate::process::scheduler::spawn_user_process(name, &path) {
        Ok(pid) => {
            // Set parent to kernel (pid 0) since this is from the terminal
            state.output.push((format!("  Process '{}' spawned (pid={})", name, pid), ACCENT_GREEN));
        }
        Err(e) => {
            state.output.push((format!("  Failed: {}", e), ACCENT_RED));
        }
    }
}

fn cmd_fds(state: &mut TerminalState, pid_str: Option<&str>) {
    let pid = match pid_str {
        Some(s) => match s.parse::<u64>() {
            Ok(p) => p,
            Err(_) => {
                state.output.push((String::from("Usage: fds [pid]"), ACCENT_RED));
                return;
            }
        },
        None => crate::process::scheduler::current_pid().unwrap_or(0),
    };
    let fds = crate::process::fd::fd_list(pid);
    state.output.push((format!("--- FDs for pid {} ---", pid), ACCENT_CYAN));
    if fds.is_empty() {
        state.output.push((String::from("  No FD table (or no FDs open)"), TEXT_SECONDARY));
    } else {
        for (fd_num, desc) in fds {
            state.output.push((format!("  fd {}: {}", fd_num, desc), TEXT_PRIMARY));
        }
    }
}

fn cmd_waitinfo(state: &mut TerminalState) {
    let count = crate::process::wait::wait_queue_count();
    let blocked = crate::process::scheduler::blocked_count();
    let snapshot = crate::process::wait::wait_queue_snapshot();
    state.output.push((String::from("--- Wait Queue ---"), ACCENT_CYAN));
    state.output.push((format!("  Wait entries:    {}", count), TEXT_PRIMARY));
    state.output.push((format!("  Blocked threads: {}", blocked), TEXT_PRIMARY));
    for (tid, reason) in snapshot.iter().take(10) {
        let reason_str = match reason {
            crate::process::wait::WaitReason::Sleep { wake_at_tick } => format!("sleep until tick {}", wake_at_tick),
            crate::process::wait::WaitReason::WaitPid { target_pid } => format!("waitpid({})", target_pid),
            crate::process::wait::WaitReason::PipeRead { pipe_id } => format!("pipe_read({})", pipe_id),
        };
        state.output.push((format!("  tid {}: {}", tid, reason_str), TEXT_SECONDARY));
    }
}

fn cmd_shutdown(state: &mut TerminalState) {
    state.output.push((String::from("Shutting down..."), ACCENT_RED));
    state.dirty = true;
    // Give GUI a moment to render the message, then shutdown
    for _ in 0..100 {
        crate::process::scheduler::yield_now();
    }
    crate::drivers::acpi::shutdown();
}

fn cmd_reboot(state: &mut TerminalState) {
    state.output.push((String::from("Rebooting..."), ACCENT_RED));
    state.dirty = true;
    for _ in 0..100 {
        crate::process::scheduler::yield_now();
    }
    crate::drivers::acpi::reboot();
}
