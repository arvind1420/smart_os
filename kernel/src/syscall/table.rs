/// Syscall number definitions.
///
/// These define the ABI between threads/processes and the kernel.

/// Total number of syscalls.
pub const SYSCALL_COUNT: usize = 71;

// ── Process syscalls (0-9) ──
pub const SYS_EXIT: usize = 0;
pub const SYS_YIELD: usize = 1;
pub const SYS_SPAWN: usize = 2;
pub const SYS_GETPID: usize = 3;
pub const SYS_SLEEP: usize = 4;
pub const SYS_FORK: usize = 5;
pub const SYS_EXEC: usize = 6;
pub const SYS_WAITPID: usize = 7;
pub const SYS_MMAP: usize = 8;
pub const SYS_PIPE: usize = 9;

// ── IPC syscalls (10-19) ──
pub const SYS_IPC_SEND: usize = 10;
pub const SYS_IPC_RECV: usize = 11;
pub const SYS_IPC_CREATE_PORT: usize = 12;
pub const SYS_IPC_LOOKUP_PORT: usize = 13;

// ── File syscalls (20-29) ──
pub const SYS_OPEN: usize = 20;
pub const SYS_CLOSE: usize = 21;
pub const SYS_READ: usize = 22;
pub const SYS_WRITE: usize = 23;
pub const SYS_STAT: usize = 24;
pub const SYS_READDIR: usize = 25;
pub const SYS_MKDIR: usize = 26;

// ── Network syscalls (30-39) — UDP ──
pub const SYS_NET_SEND: usize = 30;
pub const SYS_NET_RECV: usize = 31;
pub const SYS_NET_BIND: usize = 32;

// ── TCP syscalls (33-39) ──
pub const SYS_TCP_CONNECT: usize = 33;
pub const SYS_TCP_LISTEN: usize = 34;
pub const SYS_TCP_ACCEPT: usize = 35;
pub const SYS_TCP_SEND: usize = 36;
pub const SYS_TCP_RECV: usize = 37;
pub const SYS_TCP_CLOSE: usize = 38;
pub const SYS_TCP_STATUS: usize = 39;

// ── Knowledge Graph syscalls (40-49) ──
pub const SYS_KG_INSERT: usize = 40;
pub const SYS_KG_QUERY: usize = 41;
pub const SYS_KG_LINK: usize = 42;
pub const SYS_KG_DELETE: usize = 43;

// ── USB syscalls (50-55) ──
pub const SYS_USB_LIST_DEVICES: usize = 50;
pub const SYS_USB_DEVICE_INFO: usize = 51;
pub const SYS_USB_READ: usize = 52;
pub const SYS_USB_WRITE: usize = 53;
pub const SYS_FAT32_MOUNT: usize = 54;
pub const SYS_FAT32_SYNC: usize = 55;

// ── Extended file syscalls (56-58) ──
pub const SYS_DUP2: usize = 56;
pub const SYS_LSEEK: usize = 57;
pub const SYS_OPEN_EX: usize = 58;

// ── POSIX process syscalls (59-62) ──
pub const SYS_GETPPID: usize = 59;
pub const SYS_GETCWD: usize = 60;
pub const SYS_CHDIR: usize = 61;
pub const SYS_SELECT: usize = 62;

// ── Display server syscalls (63-64) ──
pub const SYS_DISPLAY_CMD: usize = 63;
pub const SYS_DISPLAY_EVENT: usize = 64;

// ── Package manager syscalls (65-66) ──
pub const SYS_PKG_LIST: usize = 65;
pub const SYS_PKG_INFO: usize = 66;

// ── Signal syscalls (67-68) ──
pub const SYS_SIGACTION: usize = 67;
pub const SYS_SIGRETURN: usize = 68;

// ── DNS syscalls (69) ──
pub const SYS_GETHOSTBYNAME: usize = 69;

// ── System Info syscall (70) ──
pub const SYS_SYSINFO: usize = 70;
