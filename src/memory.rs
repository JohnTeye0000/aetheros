//! Physical memory & paging.
//!
//! The CPU never touches RAM by its real (physical) address — it uses *virtual*
//! addresses, and a tree of hardware-walked tables (the **page tables**) maps
//! each 4 KiB virtual page to a physical frame of actual RAM. The bootloader
//! already built a set of page tables and switched paging on before jumping to
//! us; this module lets the kernel *read and edit* those tables itself.
//!
//! Three pieces live here:
//!   1. `init` / `active_level_4_table` — get a `Mapper` over the live tables.
//!   2. `BootInfoFrameAllocator` — hand out unused physical frames (needed
//!      whenever creating a mapping might require a fresh page table).
//!   3. `create_example_mapping` — a demo that maps a new page to the VGA buffer.

use core::sync::atomic::{AtomicU64, Ordering};

use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
    PhysAddr, VirtAddr,
};

/// Build an `OffsetPageTable` over the page tables the CPU is *currently* using.
///
/// Because the bootloader mapped all of physical memory at
/// `physical_memory_offset`, the bytes of any physical address `p` are readable
/// at virtual address `physical_memory_offset + p`. That constant offset is
/// exactly what `OffsetPageTable` needs to follow the tables down the tree.
///
/// # Safety
/// The caller must guarantee physical memory really is fully mapped at
/// `physical_memory_offset` (the bootloader's `map_physical_memory` feature
/// guarantees this) and that no other `Mapper` is alive at the same time.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    PHYS_MEM_OFFSET.store(physical_memory_offset.as_u64(), Ordering::Relaxed);
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

/// Borrow the active level-4 page table (the root of the tree).
///
/// The `CR3` control register holds the *physical* address of the level-4 table
/// the CPU is using right now. We turn that into a virtual address via the
/// offset and reborrow it as a `PageTable`.
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr // unsafe deref: trusts the offset is correct
}

/// A dead-simple physical frame allocator. It reads the bootloader's memory map,
/// walks every region marked `Usable`, and hands out 4 KiB frames one at a time.
/// It never reclaims a freed frame — it only ever moves forward. That's fine
/// until we build a real allocator; for now we just need *some* unused frames.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
    /// Frames handed back with `release`, reused before fresh ones.
    free: alloc::vec::Vec<PhysFrame>,
}

impl BootInfoFrameAllocator {
    /// # Safety
    /// The caller must guarantee the memory map is valid and that the regions
    /// marked `Usable` are genuinely unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator { memory_map, next: 0, free: alloc::vec::Vec::new() }
    }

    /// An iterator over every usable 4 KiB frame in the memory map.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        // usable regions -> their address ranges -> every 4096-aligned start
        // address within them -> the frame containing each.
        let regions = self.memory_map.iter();
        let usable = regions.filter(|r| r.region_type == MemoryRegionType::Usable);
        let addr_ranges = usable.map(|r| r.range.start_addr()..r.range.end_addr());
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

impl BootInfoFrameAllocator {
    /// Frames currently handed out (allocated minus released).
    pub fn in_use(&self) -> usize {
        self.next - self.free.len()
    }

    /// Give a frame back for reuse (e.g. when a process's address space is
    /// torn down). The caller must guarantee nothing still maps it.
    pub fn release(&mut self, frame: PhysFrame) {
        self.free.push(frame);
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if let Some(f) = self.free.pop() {
            return Some(f);
        }
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

/// Demo: map a brand-new virtual `page` onto the VGA text buffer's physical
/// frame (`0xb8000`). After this returns, writing through `page` writes to the
/// screen — proof that the kernel can author its own virtual-memory mappings.
pub fn create_example_mapping(
    page: Page,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    let frame = PhysFrame::containing_address(PhysAddr::new(0xb8000));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    // map_to is unsafe: mapping a frame that's already mapped elsewhere aliases
    // memory. Here we intentionally alias the VGA frame, for the demo only.
    let map_to_result = unsafe { mapper.map_to(page, frame, flags, frame_allocator) };
    map_to_result.expect("map_to failed").flush(); // flush() updates the TLB
}

/// Total usable physical RAM in bytes, recorded once at boot. The boot info is
/// only available in `kernel_main`, so we sum the usable regions there and stash
/// the result here for the rest of the kernel (e.g. the shell's `mem` command).
static USABLE_RAM: AtomicU64 = AtomicU64::new(0);

/// Sum every region the bootloader marked `Usable` and remember the total.
pub fn record_usable_ram(memory_map: &MemoryMap) {
    let total: u64 = memory_map
        .iter()
        .filter(|r| r.region_type == MemoryRegionType::Usable)
        .map(|r| r.range.end_addr() - r.range.start_addr())
        .sum();
    USABLE_RAM.store(total, Ordering::Relaxed);
}

/// Total usable physical RAM in bytes (0 before `record_usable_ram` runs).
pub fn usable_ram() -> u64 {
    USABLE_RAM.load(Ordering::Relaxed)
}

/// The offset at which the bootloader mapped all of physical memory, stashed
/// by `init()` so anything later in the kernel (e.g. `acpi`, hunting for a
/// BIOS table by *physical* address) can translate physical -> virtual without
/// needing its own copy of the offset threaded through every call site.
static PHYS_MEM_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Turn a physical address into the virtual address it's readable at, via the
/// bootloader's identity-plus-offset mapping of all RAM. Valid only after
/// `init()` has run.
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new(PHYS_MEM_OFFSET.load(Ordering::Relaxed) + phys.as_u64())
}

/// The kernel's page-table mapper and frame allocator, parked here after boot
/// so threads (e.g. the user-program launcher) can create mappings later on.
static KERNEL_MEM: spin::Mutex<Option<(OffsetPageTable<'static>, BootInfoFrameAllocator)>> =
    spin::Mutex::new(None);

/// Hand the mapper + frame allocator over to the global slot (call once, after
/// the heap is up).
pub fn install(mapper: OffsetPageTable<'static>, frames: BootInfoFrameAllocator) {
    // Remember the boot page tables: every process's tables are built as a copy
    // of these (the kernel half), and kernel threads run on them.
    KERNEL_CR3.store(x86_64::registers::control::Cr3::read().0.start_address().as_u64(), Ordering::SeqCst);
    *KERNEL_MEM.lock() = Some((mapper, frames));
}

/// Run `f` with exclusive access to the mapper and frame allocator. Interrupts
/// are held off for the duration so a preemption can't deadlock on the lock.
pub fn with_mem<R>(
    f: impl FnOnce(&mut OffsetPageTable<'static>, &mut BootInfoFrameAllocator) -> R,
) -> Option<R> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = KERNEL_MEM.lock();
        guard.as_mut().map(|(m, fa)| f(m, fa))
    })
}

static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// The page-table root the kernel booted with (kernel threads' address space).
pub fn kernel_cr3() -> PhysFrame {
    PhysFrame::containing_address(PhysAddr::new(KERNEL_CR3.load(Ordering::SeqCst)))
}

/// Offset at which all physical memory is mapped.
pub fn phys_offset() -> u64 {
    PHYS_MEM_OFFSET.load(Ordering::Relaxed)
}

/// Physical frames currently allocated (kernel + processes), if the manager is up.
pub fn frames_in_use() -> usize {
    with_mem(|_, fa| fa.in_use()).unwrap_or(0)
}
