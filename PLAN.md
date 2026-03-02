# Phase 5: User-Space Process Support

## Overview
Transform Smart OS from a kernel-thread-only system into a true multiprocess OS with **ring-3 user-space isolation**. One user process will run a demo program, call syscalls via the `SYSCALL` instruction, and get preempted by the timer.

## Steps (7 total, ~1500 lines new/modified)

### Step 1: Physical Frame Allocator (~180 lines)
**New:** `kernel/src/memory/frame.rs`
**Modify:** `kernel/src/memory/mod.rs`, `kernel/src/memory/heap.rs`, `kernel/src/main.rs`

- Bitmap-based allocator: each bit = one 4 KiB frame (1=free, 0=used)
- For 128 MiB RAM: 4 KB bitmap — fits easily in 1 MiB heap
- Implements `x86_64::FrameAllocator<Size4KiB>` trait for `OffsetPageTable::map_to()`
- `alloc_frame() -> Option<PhysFrame>`, `dealloc_frame(frame)`
- `heap.rs` exports `HEAP_PHYS_END` so frame allocator knows what's reserved
- Boot: init after heap, print frame stats to serial

### Step 2: Page Table Management (~250 lines)
**New:** `kernel/src/memory/paging.rs`
**Modify:** `kernel/src/memory/mod.rs`

- `init(phys_offset)` — stores physical memory offset for address translation
- `phys_to_virt(phys) -> virt` — offset-based translation
- `create_user_page_table() -> PhysFrame` — alloc new PML4, copy kernel entries 256-511
- `map_page(pml4, page, frame, flags)` — map a single page via `OffsetPageTable`
- `map_range(pml4, start, count, flags)` — allocate + map N contiguous pages
- `switch_to(pml4) -> old_frame` — write CR3
- `free_user_page_table(pml4)` — recursively free user-half pages on process exit

### Step 3: Process Model + Thread Updates (~200 lines)
**New:** `kernel/src/process/process.rs`
**Modify:** `kernel/src/process/mod.rs`, `kernel/src/process/thread.rs`

- `Process` struct: pid, name, `page_table: Option<PhysFrame>`, threads, state, exit_code
- `PROCESS_TABLE: Mutex<BTreeMap<Pid, Process>>` — global process table
- PID=0 is the kernel process for all existing kernel threads
- `Thread` gains: `pid`, `is_user`, `kernel_stack_ptr` fields
- `Thread::new_user(name, pid, entry_rip, user_rsp, priority)` — allocates kernel stack, sets up full InterruptContext frame (15 GPRs + 5 iretq words)
- Existing `Thread::new()` unchanged (sets pid=0, is_user=false)

### Step 4: SYSCALL/SYSRET Mechanism (~200 lines)
**New:** `kernel/src/arch/x86_64/syscall.rs`
**Modify:** `kernel/src/arch/x86_64/mod.rs`, `kernel/src/arch/x86_64/gdt.rs`

- Configure MSRs: EFER (SCE bit), STAR (segments), LSTAR (entry point), SFMASK (mask IF+TF)
- GDT order verified correct for SYSCALL/SYSRET:
  - STAR[47:32]=0x08 → syscall loads CS=0x08, SS=0x10
  - STAR[63:48]=0x10 → sysret loads SS=0x18|3=0x1B, CS=0x20|3=0x23
- Naked `syscall_entry`: save user RSP to static, load kernel RSP, push regs, call Rust dispatcher
- `SyscallFrame` struct, `syscall_dispatcher(frame) -> u64`
- Handles: SYS_EXIT, SYS_WRITE (to serial), SYS_YIELD, SYS_GETPID
- Export user segment selectors from GDT; add `set_tss_rsp0()` for ring transitions
- Add TSS `privilege_stack_table[0]` for interrupt stack switching from ring 3

### Step 5: ELF Binary Loader (~200 lines)
**New:** `kernel/src/process/elf.rs`
**Modify:** `kernel/src/process/mod.rs`

- Hand-written ELF64 parser (no external crate)
- `Elf64Header` + `Elf64ProgramHeader` repr(C, packed) structs
- `load_elf(elf_data, pml4_frame) -> LoadedElf { entry_point, highest_addr }`
- Iterates PT_LOAD segments, page-aligns, allocs frames, maps with correct R/W/X flags
- Copies file data into mapped frames, zeroes BSS

### Step 6: Preemptive Scheduling + Ring-3 Transitions (~200 lines modified)
**Modify:** `kernel/src/arch/x86_64/idt.rs`, `kernel/src/process/scheduler.rs`, `kernel/src/process/thread.rs`

- Replace timer handler with naked assembly stub (push 15 GPRs, call Rust, pop, iretq)
- `InterruptContext` struct (15 GPRs + 5 CPU-pushed words)
- Timer Rust handler: tick + every 10 ticks call `preempt_with_context()`
- Use `set_handler_addr()` for raw handler in IDT
- Scheduler: `preempt_with_context(ctx)` saves user thread state, picks next
- New `switch_to_user_thread(old_sp, new_sp, new_cr3)` — naked asm: save kernel regs, switch CR3, pop 15 GPRs, iretq
- Before user switch: update TSS RSP0 + SYSCALL kernel RSP
- Kernel threads continue using existing cooperative `thread_switch`

### Step 7: User-Space Demo + Integration (~150 lines)
**New:** `kernel/src/process/userspace.rs`
**Modify:** `kernel/src/main.rs`

- `create_hello_elf() -> Vec<u8>` — constructs minimal ELF64 in memory
  - Machine code: `syscall(SYS_WRITE, 1, "Hello from user space!\n", 23)` then `syscall(SYS_EXIT, 0)`
  - Valid ELF64 header + PT_LOAD at vaddr 0x400000
- `spawn_user_process(name, elf_path)` — read ELF → create page table → load → map user stack (64KB at 0x7FFF_FFFF_0000) → create Process + Thread → enqueue
- Boot: VFS mkdir `/bin`, store ELF, spawn process, version → v0.5.0

## Key Architectural Decisions

1. **Static per-CPU data instead of SWAPGS** — simpler for single-CPU; syscall entry/exit uses a known static for user RSP save/load
2. **Kernel half shared via PML4 entries 256-511** — all user page tables clone the kernel's upper-half entries
3. **Dual context switch paths** — kernel threads use existing `thread_switch` (callee-saved only); user threads use full `InterruptContext` save/restore with CR3 switch
4. **TSS RSP0 per-thread** — updated before switching to user thread so interrupts from ring-3 land on correct kernel stack
5. **User pointer validation** — range check only for MVP (< 0x0000_8000_0000_0000)

## Expected Serial Output
```
[frame] Frame allocator: XXXXX free / XXXXX total frames
[paging] Page table management initialized
[process] Kernel process (pid=0) registered
[syscall] SYSCALL/SYSRET configured (LSTAR=0x...)
[boot] User process 'hello' spawned (pid=1)
Hello from user space!
[process] Process 1 exited with code 0
```
