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
pub mod browser;
pub mod browser_render;
pub mod browser_js;
pub mod login;
pub mod lockscreen;
pub mod media_player;
pub mod code_editor;
pub mod dashboard;
pub mod image_viewer;
pub mod pdf_reader;
pub mod video_player;
pub mod email_client;
pub mod office;
pub mod browser_v2;
pub mod usb_storage;
pub mod app_store;
pub mod power_manager;
pub mod crash_recovery;
pub mod setup_wizard;
pub mod accessibility;
pub mod printer;
pub mod update_manager;
pub mod cloud_sync;
pub mod gaming;
pub mod browser_ipc;
pub mod browser_sandbox;
pub mod browser_persist;
pub mod browser_downloads;
pub mod browser_wpt;
