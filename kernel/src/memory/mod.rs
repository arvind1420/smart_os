/// Memory management for Smart OS kernel.
///
/// Phase 1: Kernel heap allocator using linked_list_allocator.
/// Phase 5: Frame allocator, virtual memory manager, paging.
/// Phase 10: CoW fork, shared memory, swap/page fault handler, ASLR.

pub mod heap;
#[allow(dead_code)]
pub mod frame;
#[allow(dead_code)]
pub mod paging;
#[allow(dead_code)]
pub mod cow;
#[allow(dead_code)]
pub mod shmem;
#[allow(dead_code)]
pub mod swap;
#[allow(dead_code)]
pub mod aslr;
pub mod distributed;
