/// /proc virtual filesystem for Smart OS.
///
/// Generates synthetic files that Linux binaries read for system information.
/// Files are pre-written into the VFS at boot; dynamic entries (/proc/<pid>/*)
/// are regenerated on each read via the VFS stat hook.

use alloc::format;
use alloc::string::String;

/// Mount all /proc entries into the VFS.
pub fn mount() {
    write_cpuinfo();
    write_meminfo();
    write_version();
    write_stat();
    write_uptime();
    write_mounts();
    write_net();
    write_self();
    crate::serial_println!("[proc_fs] /proc mounted.");
}

fn w(path: &str, data: &[u8]) {
    let _ = crate::vfs::create_and_write(path, data);
}

fn ws(path: &str, s: &str) {
    w(path, s.as_bytes());
}

// ── /proc/cpuinfo ─────────────────────────────────────────────────────────

fn write_cpuinfo() {
    let cpu_count = crate::arch::x86_64::smp::cpu_count();
    let mut out = String::new();
    for i in 0..cpu_count as u32 {
        out.push_str(&format!(
"processor\t: {i}
vendor_id\t: GenuineIntel
cpu family\t: 6
model\t\t: 142
model name\t: Smart OS Virtual CPU @ 2.40GHz
stepping\t: 10
microcode\t: 0xffffffff
cpu MHz\t\t: 2400.000
cache size\t: 8192 KB
physical id\t: 0
siblings\t: {cpu_count}
core id\t\t: {i}
cpu cores\t: {cpu_count}
apicid\t\t: {i}
initial apicid\t: {i}
fpu\t\t: yes
fpu_exception\t: yes
cpuid level\t: 22
wp\t\t: yes
flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc rep_good nopl xtopology nonstop_tsc eagerfpu pni pclmulqdq ssse3 fma cx16 pcid sse4_1 sse4_2 x2apic movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp fsgsbase tsc_adjust bmi1 avx2 smep bmi2 erms invpcid mpx rdseed adx smap clflushopt intel_pt xsaveopt xsavec xgetbv1 dtherm ida arat pln pts hwp hwp_notify hwp_act_window hwp_epp md_clear flush_l1d arch_capabilities
bugs\t\t: spectre_v1 spectre_v2 spec_store_bypass mds swapgs taa itlb_multihit srbds
bogomips\t: 4800.00
clflush size\t: 64
cache_alignment\t: 64
address sizes\t: 39 bits physical, 48 bits virtual
power management:

"));
    }
    ws("/proc/cpuinfo", &out);
}

// ── /proc/meminfo ─────────────────────────────────────────────────────────

fn write_meminfo() {
    let (used, free) = crate::memory::heap::heap_stats();
    let total_kb = (used + free) / 1024;
    let free_kb  = free / 1024;
    let avail_kb = free_kb;
    ws("/proc/meminfo", &format!(
"MemTotal:       {total_kb} kB
MemFree:        {free_kb} kB
MemAvailable:   {avail_kb} kB
Buffers:               0 kB
Cached:                0 kB
SwapCached:            0 kB
Active:                0 kB
Inactive:              0 kB
Active(anon):          0 kB
Inactive(anon):        0 kB
Active(file):          0 kB
Inactive(file):        0 kB
Unevictable:           0 kB
Mlocked:               0 kB
SwapTotal:             0 kB
SwapFree:              0 kB
Dirty:                 0 kB
Writeback:             0 kB
AnonPages:             0 kB
Mapped:                0 kB
Shmem:                 0 kB
Slab:                  0 kB
SReclaimable:          0 kB
SUnreclaim:            0 kB
KernelStack:        8192 kB
PageTables:            0 kB
NFS_Unstable:          0 kB
Bounce:                0 kB
WritebackTmp:          0 kB
CommitLimit:    {total_kb} kB
Committed_AS:          0 kB
VmallocTotal: 34359738367 kB
VmallocUsed:           0 kB
VmallocChunk:          0 kB
HardwareCorrupted:     0 kB
AnonHugePages:         0 kB
ShmemHugePages:        0 kB
ShmemPmdMapped:        0 kB
HugePages_Total:       0
HugePages_Free:        0
HugePages_Rsvd:        0
HugePages_Surp:        0
Hugepagesize:       2048 kB
DirectMap4k:       65536 kB
DirectMap2M:     {total_kb} kB
DirectMap1G:           0 kB
"));
}

// ── /proc/version ─────────────────────────────────────────────────────────

fn write_version() {
    ws("/proc/version",
       "Linux version 5.15.0-smartos (smartos@build) (gcc version 12.0) #1 SMP Smart OS 0.12.0\n");
    ws("/proc/sys/kernel/ostype",    "Linux\n");
    ws("/proc/sys/kernel/osrelease", "5.15.0-smartos\n");
    ws("/proc/sys/kernel/version",   "#1 SMP Smart OS 0.12.0\n");
    ws("/proc/sys/kernel/hostname",  "smartos\n");
}

// ── /proc/stat ────────────────────────────────────────────────────────────

fn write_stat() {
    let ticks = crate::drivers::timer::ticks();
    ws("/proc/stat", &format!(
"cpu  {ticks} 0 0 0 0 0 0 0 0 0
cpu0 {ticks} 0 0 0 0 0 0 0 0 0
intr 0
ctxt 0
btime 1700000000
processes 1
procs_running 1
procs_blocked 0
softirq 0 0 0 0 0 0 0 0 0 0 0
"));
}

// ── /proc/uptime ──────────────────────────────────────────────────────────

fn write_uptime() {
    let ticks = crate::drivers::timer::ticks();
    let uptime_secs = ticks / 100; // 100 Hz timer
    ws("/proc/uptime", &format!("{uptime_secs}.00 {uptime_secs}.00\n"));
}

// ── /proc/mounts ──────────────────────────────────────────────────────────

fn write_mounts() {
    ws("/proc/mounts",
"sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
devtmpfs /dev devtmpfs rw,nosuid,size=8192k,nr_inodes=2048,mode=755 0 0
tmpfs /tmp tmpfs rw,nosuid,nodev 0 0
smartfs / smartfs rw,relatime 0 0
");
    // /proc/self/mounts is typically a symlink to /proc/mounts
    ws("/proc/self/mounts",
"smartfs / smartfs rw,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
devtmpfs /dev devtmpfs rw,nosuid 0 0
tmpfs /tmp tmpfs rw,nosuid,nodev 0 0
");
}

// ── /proc/net ─────────────────────────────────────────────────────────────

fn write_net() {
    let ip = crate::net::LOCAL_IP;
    ws("/proc/net/dev",
"Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
  eth0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
");
    ws("/proc/net/if_inet6", "");
    ws("/proc/net/route",
"Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n");
    ws("/proc/net/tcp",  "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
    ws("/proc/net/udp",  "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
    ws("/proc/net/unix", "Num       RefCount Protocol Flags    Type St Inode Path\n");
    ws("/proc/sys/net/ipv4/ip_forward", "0\n");
    ws("/proc/sys/net/core/somaxconn", "128\n");
}

// ── /proc/self ────────────────────────────────────────────────────────────

fn write_self() {
    // These are regenerated per-process in handle_proc_self_read().
    // Here we write placeholders that work for single-process use.
    w("/proc/self/cmdline",  b"smartos\0");
    w("/proc/self/environ",  b"HOME=/root\0PATH=/usr/bin:/bin:/usr/local/bin\0TERM=xterm\0LANG=C.UTF-8\0");
    ws("/proc/self/status",
"Name:\tsmartd
State:\tS (sleeping)
Tgid:\t1
Ngid:\t0
Pid:\t1
PPid:\t0
TracerPid:\t0
Uid:\t0\t0\t0\t0
Gid:\t0\t0\t0\t0
FDSize:\t64
Groups:\t0
VmPeak:\t   65536 kB
VmSize:\t   32768 kB
VmLck:\t       0 kB
VmPin:\t       0 kB
VmHWM:\t    8192 kB
VmRSS:\t    8192 kB
VmData:\t    4096 kB
VmStk:\t     128 kB
VmExe:\t     256 kB
VmLib:\t    8192 kB
VmPTE:\t      32 kB
VmSwap:\t       0 kB
Threads:\t1
SigQ:\t0/15360
SigPnd:\t0000000000000000
ShdPnd:\t0000000000000000
SigBlk:\t0000000000000000
SigIgn:\t0000000000000001
SigCgt:\t0000000000000000
CapInh:\t0000000000000000
CapPrm:\t000001ffffffffff
CapEff:\t000001ffffffffff
CapBnd:\t000001ffffffffff
CapAmb:\t0000000000000000
NoNewPrivs:\t0
Seccomp:\t0
Cpus_allowed:\tff
Cpus_allowed_list:\t0-7
Mems_allowed:\t00000000,00000001
Mems_allowed_list:\t0
voluntary_ctxt_switches:\t100
nonvoluntary_ctxt_switches:\t10
");
    ws("/proc/self/maps",
"00400000-00401000 r-xp 00000000 00:00 0\t/bin/smartos
7fff00000000-7fff00001000 rwxp 00000000 00:00 0\t[stack]
7ffff7ffd000-7ffff7fff000 r--p 00000000 00:00 0\t[vvar]
7ffff7fff000-7ffff8000000 r-xp 00000000 00:00 0\t[vdso]
ffffffffff600000-ffffffffff601000 r-xp 00000000 00:00 0\t[vsyscall]
");
    ws("/proc/self/exe",    "/bin/smartos");
    ws("/proc/self/cwd",    "/");
    ws("/proc/self/root",   "/");
    ws("/proc/self/loginuid", "0\n");
    ws("/proc/self/wchan",  "0\n");

    // /proc/self/fd/ — symlinks for stdin/stdout/stderr
    ws("/proc/self/fd/0",   "/dev/stdin");
    ws("/proc/self/fd/1",   "/dev/stdout");
    ws("/proc/self/fd/2",   "/dev/stderr");

    // Additional /proc/sys entries many programs read
    ws("/proc/sys/kernel/pid_max",        "32768\n");
    ws("/proc/sys/kernel/threads-max",    "32768\n");
    ws("/proc/sys/kernel/ngroups_max",    "65536\n");
    ws("/proc/sys/kernel/cap_last_cap",   "40\n");
    ws("/proc/sys/kernel/random/entropy_avail", "3072\n");
    ws("/proc/sys/kernel/random/uuid",    "550e8400-e29b-41d4-a716-446655440000\n");
    ws("/proc/sys/vm/overcommit_memory",  "0\n");
    ws("/proc/sys/vm/max_map_count",      "65530\n");
    ws("/proc/sys/fs/file-max",           "1048576\n");
    ws("/proc/sys/fs/pipe-max-size",      "1048576\n");
    ws("/proc/sys/fs/nr_open",            "1048576\n");
}

/// Regenerate per-process /proc/<pid>/* entries.
/// Called from the syscall layer when a process executes or exits.
pub fn update_process_entry(pid: u64, name: &str) {
    let base = format!("/proc/{pid}");
    let _ = crate::vfs::mkdir(&base);

    ws(&format!("{base}/cmdline"), name);
    ws(&format!("{base}/status"), &format!(
"Name:\t{name}
State:\tS (sleeping)
Tgid:\t{pid}
Pid:\t{pid}
PPid:\t0
Uid:\t0\t0\t0\t0
Gid:\t0\t0\t0\t0
VmPeak:\t   16384 kB
VmSize:\t    8192 kB
VmRSS:\t    4096 kB
Threads:\t1
"));
    ws(&format!("{base}/maps"),   "");
    ws(&format!("{base}/exe"),    &format!("/bin/{name}"));
    ws(&format!("{base}/cwd"),    "/");
    ws(&format!("{base}/fd/0"),   "/dev/stdin");
    ws(&format!("{base}/fd/1"),   "/dev/stdout");
    ws(&format!("{base}/fd/2"),   "/dev/stderr");
}

/// Remove /proc/<pid> entries when a process exits.
pub fn remove_process_entry(pid: u64) {
    // VFS doesn't have recursive delete yet; remove the key files.
    let base = format!("/proc/{pid}");
    for file in &["cmdline", "status", "maps", "exe", "cwd"] {
        let _ = crate::vfs::unlink(&format!("{base}/{file}"));
    }
}
