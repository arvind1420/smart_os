/// Smart OS Applications — Phase 4 interactive apps.
///
/// Each application runs as a kernel thread, owns state via a module-level
/// static Mutex, and creates its own window in the desktop environment.
/// Phase 8: Text editor with RawInput widget.

pub mod terminal;
pub mod file_manager;
pub mod sysmon;
pub mod editor;
pub mod calculator;
pub mod task_manager;
pub mod settings;
pub mod pkgmgr;
