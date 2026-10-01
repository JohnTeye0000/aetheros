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
//! Stage 5 built a **bump allocator** here — the simplest design there is, but
//! one that could only reclaim memory once *every* allocation was freed at
//! once. This stage replaces it with a **free-list allocator**: individual
//! blocks can now be freed and reused independently. See `LinkedListAllocator`
//! below for how it works.

use core::alloc::{GlobalAlloc, Layout};
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;
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
pub const HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MiB: thread stacks for many processes + a 1 MiB filesystem image

/// The global allocator instance. `#[global_allocator]` tells Rust to route all
/// `alloc` allocations through it. It starts empty; `init_heap` fills it in.
#[global_allocator]
static ALLOCATOR: Locked<LinkedListAllocator> = Locked::new(LinkedListAllocator::new());

/// How many allocations are currently live — incremented on a successful
/// `alloc`, decremented on every `dealloc`. Purely for the `mem` shell command;
/// the allocator itself doesn't need this to function.
static LIVE_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

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

/// One free block of heap memory, described by a node **stored inside the
/// block itself** — freeing memory costs nothing extra because the freed
/// memory *is* the bookkeeping. `size` is how big the block is; `next` links
/// to the next free block, forming a singly-linked list threaded through
/// whatever memory happens to be unused right now.
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    const fn new(size: usize) -> Self {
        ListNode { size, next: None }
    }
    fn start_addr(&self) -> usize {
        self as *const Self as usize
    }
    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

/// A **free-list allocator**: instead of one bump pointer, it keeps a linked
/// list of every free block, sorted by address, merging neighbouring blocks
/// whenever a free makes them touch (coalescing).
///
/// **Allocating** walks the list looking for the first block big enough (and
/// with a usable aligned start) to hold the request — "first fit". If the
/// block is bigger than needed, the leftover tail becomes a new, smaller free
/// block instead of being wasted.
///
/// **Freeing** turns the returned memory back into a `ListNode` and inserts it
/// in address order, merging it with the free blocks on either side if they
/// touch. No bookkeeping table elsewhere — the freed memory becomes its own
/// record.
///
/// The real gain over Stage 5's bump allocator: freed space is reused
/// individually. A bump allocator could only reclaim space once *everything*
/// was freed at once.
pub struct LinkedListAllocator {
    head: ListNode,
}

impl LinkedListAllocator {
    pub const fn new() -> Self {
        LinkedListAllocator {
            head: ListNode::new(0),
        }
    }

    /// # Safety
    /// The caller must guarantee the given range is unused and stays valid,
    /// and must call this exactly once before any allocation.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.add_free_region(heap_start, heap_size);
    }

    /// Return a block of memory to the free list (also used to seed the heap).
    ///
    /// Stage 35 fix: the list is kept **sorted by address**, and a freed block
    /// is **merged with its neighbours** when they touch. Before this, every
    /// free was pushed onto the front unmerged, so after thousands of
    /// alloc/free cycles the heap was shredded into small pieces: plenty of
    /// free bytes in total, but no single piece big enough for a 32 KiB thread
    /// stack. That killed long benchmark runs with "memory allocation of
    /// 32768 bytes failed" -- see devlog 035.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        // A block has to be big enough to hold a ListNode (it's about to
        // become one) and correctly aligned for one.
        assert_eq!(align_up(addr, mem::align_of::<ListNode>()), addr);
        assert!(size >= mem::size_of::<ListNode>());

        // Walk to the last node that starts before `addr`. (Raw pointers: the
        // list is threaded through memory we own, and we need to look at
        // `prev` and `prev.next` at the same time.)
        let head: *mut ListNode = &mut self.head;
        let mut prev: *mut ListNode = head;
        while let Some(next) = (*prev).next.as_mut() {
            let next_ptr: *mut ListNode = &mut **next;
            if (*next_ptr).start_addr() > addr {
                break;
            }
            prev = next_ptr;
        }

        // Merge with the following block if it starts exactly where we end.
        let mut size = size;
        let mut after = (*prev).next.take();
        if let Some(n) = after.as_mut() {
            if addr + size == n.start_addr() {
                size += n.size;
                after = n.next.take();
            }
        }

        // Merge into the preceding block if it ends exactly where we start
        // (the head is a sentinel, not real memory, so never merge into it).
        if prev != head && (*prev).end_addr() == addr {
            (*prev).size += size;
            (*prev).next = after;
        } else {
            let node_ptr = addr as *mut ListNode;
            node_ptr.write(ListNode { size, next: after });
            (*prev).next = Some(&mut *node_ptr);
        }
    }

    /// Size of the largest single free block (fragmentation diagnostic).
    fn largest_free(&self) -> usize {
        let mut best = 0;
        let mut current = &self.head;
        while let Some(ref region) = current.next {
            best = best.max(region.size);
            current = region;
        }
        best
    }

    /// First-fit search: walk the list, and for the first block that can
    /// actually satisfy `size`/`align` (via `alloc_from_region`), unlink it
    /// from the list and return it along with where the allocation should
    /// start within it.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut current = &mut self.head;
        while let Some(ref mut region) = current.next {
            if let Ok(alloc_start) = Self::alloc_from_region(region, size, align) {
                // Found one — splice it out of the list before handing it back.
                let next = region.next.take();
                let region = current.next.take().unwrap();
                current.next = next;
                return Some((region, alloc_start));
            } else {
                current = current.next.as_mut().unwrap();
            }
        }
        None
    }

    /// Check whether `region` can hold `size` bytes at the given `align`, and
    /// if so, where the allocation would have to start. Fails if the region is
    /// too small, OR if it's big enough but the *leftover* tail after the
    /// allocation would be too small to become a `ListNode` of its own (we
    /// have no way to track leftover space smaller than one node).
    fn alloc_from_region(region: &ListNode, size: usize, align: usize) -> Result<usize, ()> {
        let alloc_start = align_up(region.start_addr(), align);
        let alloc_end = alloc_start.checked_add(size).ok_or(())?;
        if alloc_end > region.end_addr() {
            return Err(()); // too small
        }
        let excess_size = region.end_addr() - alloc_end;
        if excess_size > 0 && excess_size < mem::size_of::<ListNode>() {
            return Err(()); // leftover tail can't hold a ListNode
        }
        Ok(alloc_start)
    }

    /// `GlobalAlloc` gives us a `Layout` (size + alignment); every block we
    /// hand out or take back must be at least big enough and aligned enough to
    /// later hold a `ListNode`, since it might become one when freed.
    fn size_align(layout: Layout) -> (usize, usize) {
        let layout = layout
            .align_to(mem::align_of::<ListNode>())
            .expect("adjusting alignment failed")
            .pad_to_align();
        let size = layout.size().max(mem::size_of::<ListNode>());
        (size, layout.align())
    }

    /// Sum every free block currently in the list — used by the `mem` shell
    /// command to report usage (`used = total - free`), computed fresh each
    /// time rather than tracked incrementally, so it can never drift out of
    /// sync with the list itself.
    fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut current = &self.head;
        while let Some(ref region) = current.next {
            total += region.size;
            current = region;
        }
        total
    }
}

// GlobalAlloc's methods take `&self`, but the allocator must mutate the free
// list. We can't implement the foreign `GlobalAlloc` trait on a foreign
// `Mutex` (the orphan rule), so we wrap the mutex in our own `Locked` type and
// implement it on that.
unsafe impl GlobalAlloc for Locked<LinkedListAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Interrupts stay off while the lock is held. Otherwise a timer
        // interrupt arriving mid-allocation whose handler also allocates
        // (the scheduler records latency samples) would spin forever on a lock
        // its own CPU holds. That exact deadlock hung long benchmark runs
        // after ~70 runs -- see devlog 035.
        interrupts::without_interrupts(|| self.alloc_inner(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        interrupts::without_interrupts(|| self.dealloc_inner(ptr, layout))
    }
}

impl Locked<LinkedListAllocator> {
    unsafe fn alloc_inner(&self, layout: Layout) -> *mut u8 {
        let (size, align) = LinkedListAllocator::size_align(layout);
        let mut allocator = self.lock();

        if let Some((region, alloc_start)) = allocator.find_region(size, align) {
            let alloc_end = alloc_start.checked_add(size).expect("overflow");
            let excess_size = region.end_addr() - alloc_end;
            if excess_size > 0 {
                // Leftover space after this allocation becomes its own free
                // block, rather than being wasted.
                allocator.add_free_region(alloc_end, excess_size);
            }
            LIVE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            alloc_start as *mut u8
        } else {
            ptr::null_mut() // no region big enough — out of memory
        }
    }

    unsafe fn dealloc_inner(&self, ptr: *mut u8, layout: Layout) {
        let (size, _) = LinkedListAllocator::size_align(layout);
        self.lock().add_free_region(ptr as usize, size);
        LIVE_ALLOCATIONS.fetch_sub(1, Ordering::Relaxed);
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
/// "In use" is computed as `total - free`, walking the free list fresh each
/// call — so it's always an honest reflection of the list's actual state, not
/// a separately-tracked number that could drift out of sync with it.
pub fn heap_stats() -> (usize, usize, usize) {
    interrupts::without_interrupts(|| {
        let allocator = ALLOCATOR.lock();
        let total = HEAP_SIZE;
        let free = allocator.free_bytes();
        let used = total.saturating_sub(free);
        (used, total, LIVE_ALLOCATIONS.load(Ordering::Relaxed))
    })
}

/// Size of the largest single free block: when this is much smaller than the
/// total free space, the heap is fragmented.
pub fn largest_free_block() -> usize {
    interrupts::without_interrupts(|| ALLOCATOR.lock().largest_free())
}
