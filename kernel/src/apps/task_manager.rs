/// Smart OS Task Manager — Live thread/process viewer.
///
/// Displays all running threads with TID, name, state, and priority.
/// Auto-refreshes periodically and supports line selection.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;
use crate::process::ThreadState;

/// Task manager application state.
pub struct TaskManagerState {
    pub window_id: WindowId,
    /// Currently selected thread TID (from line click).
    pub selected_tid: Option<u64>,
    /// Whether state changed and needs GUI sync.
    pub dirty: bool,
}

pub static STATE: Mutex<Option<TaskManagerState>> = Mutex::new(None);

/// Task manager thread entry point.
pub fn run() {
    // Create the task manager window
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() {
            Some(d) => d,
            None => return,
        };

        let mut win = Window::new("Tasks", 540, 60, 400, 340, ACCENT_ORANGE);
        win.use_widgets = true;

        // Widget 0: Scrollable thread list (main area)
        let list_widget = Widget::new(0, 0, 0, 400, 280,
            WidgetKind::ScrollText(ScrollableText::new(200)));

        // Widget 1: Status bar label (bottom)
        let status_widget = Widget::new(1, 0, 284, 400, 20,
            WidgetKind::Label(StaticLabel::new("Loading...", TEXT_SECONDARY)));

        win.add_widget(list_widget);
        win.add_widget(status_widget);

        let id = win.id;
        desk.wm.add(win);
        id
    };

    // Initialize state
    *STATE.lock() = Some(TaskManagerState {
        window_id,
        selected_tid: None,
        dirty: true,
    });

    // Main loop: auto-refresh and poll for interactions
    let mut refresh_counter: u32 = 0;

    loop {
        // Poll for widget actions (e.g. line clicks)
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            match action {
                WidgetAction::Execute(AppCommand::LineClicked(line_idx)) => {
                    // Line 0 and 1 are header/separator — data starts at line 2
                    if line_idx >= 2 {
                        let threads = crate::process::scheduler::list_threads();
                        let data_idx = line_idx - 2;
                        if data_idx < threads.len() {
                            let mut state = STATE.lock();
                            if let Some(ref mut s) = *state {
                                let tid = threads[data_idx].0;
                                // Toggle selection
                                if s.selected_tid == Some(tid) {
                                    s.selected_tid = None;
                                } else {
                                    s.selected_tid = Some(tid);
                                }
                                s.dirty = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Auto-refresh every ~100 yields
        refresh_counter += 1;
        if refresh_counter >= 100 {
            refresh_counter = 0;
            let mut state = STATE.lock();
            if let Some(ref mut s) = *state {
                s.dirty = true;
            }
        }

        // Yield CPU
        for _ in 0..5 {
            crate::process::scheduler::yield_now();
        }
    }
}

/// Sync task manager state to window widgets (called from render loop).
pub fn sync_to_window(window: &mut Window) {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };
    if !s.dirty { return; }
    s.dirty = false;

    // Gather thread info
    let threads = crate::process::scheduler::list_threads();
    let selected = s.selected_tid;

    // Build display lines
    let mut lines: Vec<(String, Color)> = Vec::new();

    // Header
    lines.push((
        String::from(" TID  Name             State    Pri"),
        ACCENT_ORANGE,
    ));
    lines.push((
        String::from(" ---  ---------------  -------  ---"),
        TEXT_MUTED,
    ));

    // Thread rows
    let mut running_count: usize = 0;
    let mut ready_count: usize = 0;

    for (tid, name, tstate, priority) in &threads {
        let (state_str, color) = match tstate {
            ThreadState::Running => {
                running_count += 1;
                ("Running", ACCENT_GREEN)
            }
            ThreadState::Ready => {
                ready_count += 1;
                ("Ready  ", TEXT_PRIMARY)
            }
            ThreadState::Blocked => ("Blocked", ACCENT_BLUE),
            ThreadState::Dead => ("Dead   ", ACCENT_RED),
        };

        // Truncate or pad name to 15 chars
        let display_name = if name.len() > 15 {
            &name[..15]
        } else {
            name.as_str()
        };

        let line = format!(" {:>3}  {:<15}  {}  {:>3}", tid, display_name, state_str, priority);

        // Highlight selected thread
        let line_color = if selected == Some(*tid) {
            ACCENT_ORANGE
        } else {
            color
        };

        lines.push((line, line_color));
    }

    let total = threads.len();
    let status = format!(
        " Threads: {}  |  Running: {}  Ready: {}",
        total, running_count, ready_count
    );

    // Update ScrollText widget (id=0)
    if let Some(widget) = window.get_widget_mut(0) {
        if let WidgetKind::ScrollText(ref mut scroll) = widget.kind {
            scroll.lines = lines;
        }
    }

    // Update Label widget (id=1)
    if let Some(widget) = window.get_widget_mut(1) {
        if let WidgetKind::Label(ref mut label) = widget.kind {
            label.text = status;
            label.color = TEXT_SECONDARY;
        }
    }
}
