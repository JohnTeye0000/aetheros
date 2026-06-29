//! The kernel heap — with an allocator we wrote ourselves.
//!
//! Up to now the kernel could only use fixed, compile-time memory — no `Box`,
//! no `Vec`, no `String`. Those need a *heap*: a region of memory an allocator
//! carves up on demand. We already have paging (Stage 4), so a heap is:
//!
//!   1. Pick a virtual range and **map** every page in it to real RAM.
//!   2. Hand that range to an allocator registered as the `#[global_allocator]`,
//!      so every `Box`/`Vec`/`String` allocation routes through it.
//!
//! The allocator here is a hand-written **bump allocator** — the simplest design
//! there is. See `BumpAllocator` below for how (and why) it works.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

/// Where the heap lives in virtual memory, and how big it is. The address is
/// arbitrary — a chunk of the vast unused virtual space that won't collide with
/// the kernel, the stack, or the physical-memory map.
pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 100 * 1024; // 100 KiB

/// The global allocator instance. `#[global_allocator]` tells Rust to route all
/// `alloc` allocations through it. It starts empty; `init_heap` fills it in.
#[global_allocator]
static ALLOCATOR: Locked<BumpAllocator> = Locked::new(BumpAllocator::new());

/// Map the heap's pages to physical frames, then hand the region to the
/// allocator. Call once, after paging is up, before any heap allocation.
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    // The inclusive range of pages covering [HEAP_START, HEAP_START + HEAP_SIZE).
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    // Back each heap page with a fresh physical frame, mapped writable.
    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    // Now that the memory is real and writable, give it to the allocator.
    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

/// A **bump allocator**: the simplest possible heap allocator.
///
/// It keeps a single pointer, `next`, starting at the bottom of the heap. Each
/// allocation rounds `next` up to the required alignment, hands back that
/// address, and bumps `next` past the new block. Allocation is therefore just a
/// pointer add — extremely fast.
///
/// The catch is freeing: a bump allocator can't reclaim an individual block,
/// because it only remembers one pointer. So it counts live allocations and
/// only resets `next` to the start once that count hits zero (everything freed).
/// That's a real limitation — a smarter allocator (free lists) is a later
/// milestone — but it's perfect for understanding the core idea.
pub struct BumpAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
    allocations: usize,
}

impl BumpAllocator {
    pub const fn new() -> Self {
        BumpAllocator {
            heap_start: 0,
            heap_end: 0,
            next: 0,
            allocations: 0,
        }
    }

    /// # Safety
    /// The caller must guarantee the given range is unused and stays valid, and
    /// must call this exactly once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_end = heap_start.saturating_add(heap_size);
        self.next = heap_start;
    }
}

// GlobalAlloc's methods take `&self`, but a bump allocator must mutate `next`.
// We can't implement the foreign `GlobalAlloc` trait on a foreign `Mutex` (the
// orphan rule), so we wrap the mutex in our own `Locked` type and implement it
// on that.
unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut bump = self.lock();

        let alloc_start = align_up(bump.next, layout.align());
        let alloc_end = match alloc_start.checked_add(layout.size()) {
            Some(end) => end,
            None => return ptr::null_mut(), // overflow
        };

        if alloc_end > bump.heap_end {
            ptr::null_mut() // out of memory
        } else {
            bump.next = alloc_end;
            bump.allocations += 1;
            alloc_start as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        let mut bump = self.lock();
        bump.allocations -= 1;
        // Only reclaim space once every allocation has been freed.
        if bump.allocations == 0 {
            bump.next = bump.heap_start;
        }
    }
}

/// A thin wrapper around `spin::Mutex` so we can implement the foreign
/// `GlobalAlloc` trait on our own type (the orphan rule forbids implementing it
/// directly on `spin::Mutex`).
pub struct Locked<A> {
    inner: Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<A> {
        self.inner.lock()
    }
}

/// Round `addr` up to the next multiple of `align` (which is always a power of
/// two for a `Layout`). The bit trick: `(addr + align - 1) & !(align - 1)`.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// Report heap usage: `(bytes_in_use, total_bytes, live_allocations)`.
///
/// "In use" is the bump pointer's distance from the heap start — its
/// high-water mark. Remember a bump allocator only reclaims space once *every*
/// allocation has been freed (it then resets `next` to the start), so this is
/// the honest picture of how a bump allocator actually behaves.
pub fn heap_stats() -> (usize, usize, usize) {
    let bump = ALLOCATOR.lock();
    let used = bump.next.saturating_sub(bump.heap_start);
    let total = bump.heap_end.saturating_sub(bump.heap_start);
    (used, total, bump.allocations)
}
