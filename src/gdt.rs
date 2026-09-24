//! Global Descriptor Table (GDT) + Task State Segment (TSS).
//!
//! The GDT is a leftover from the 16/32-bit era that x86-64 still requires at
//! boot: a table of "segments". We barely use segmentation, but we need a valid
//! GDT to hold a **TSS**, and the TSS is what we actually care about here.
//!
//! The TSS carries an **Interrupt Stack Table (IST)** — up to 7 known-good
//! stacks the CPU can *unconditionally switch to* when a chosen exception fires.
//! That solves a nasty problem: if the kernel stack overflows, the CPU faults
//! while trying to push the exception frame... onto the overflowed stack... which
//! faults again → double fault → and if the double-fault handler *also* runs on
//! that broken stack, it triple-faults and the machine reboots. Putting the
//! double-fault handler on its own IST stack breaks the cycle: it always has a
//! clean stack to run on, so it can report the problem instead of rebooting.

use lazy_static::lazy_static;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Which IST slot we dedicate to the double-fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// The TSS. `static mut` (rather than a lazily-built immutable) because the
/// kernel stack pointer used for ring 3 -> ring 0 transitions (`rsp0`) is
/// rewritten each time a user program starts; the CPU reads it from memory on
/// every such transition.
static mut TSS: TaskStateSegment = TaskStateSegment::new();

const DF_STACK_SIZE: usize = 4096 * 5; // 20 KiB is plenty for a handler
static mut DF_STACK: [u8; DF_STACK_SIZE] = [0; DF_STACK_SIZE];

/// The segment selectors we need after loading the GDT.
struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    user_code: SegmentSelector,
    user_data: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        // Order matters for SYSCALL/SYSRET, which derive selectors from one
        // base value: kernel code (0x08), kernel data (0x10), then user data
        // (0x18) and user code (0x20) -- SYSRET wants data at base+8, code at base+16.
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let data_selector = gdt.add_entry(Descriptor::kernel_data_segment());
        let user_data = gdt.add_entry(Descriptor::user_data_segment());
        let user_code = gdt.add_entry(Descriptor::user_code_segment());
        // SAFETY: TSS lives for the whole run; the CPU only reads it.
        let tss_selector =
            gdt.add_entry(Descriptor::tss_segment(unsafe { &*core::ptr::addr_of!(TSS) }));
        (gdt, Selectors { code_selector, data_selector, tss_selector, user_code, user_data })
    };
}

/// Load our GDT, point the CPU's code segment at our entry, and load the TSS.
/// Must run before the IDT is set up, since the IDT references the IST index.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    unsafe {
        // Stacks grow downward, so the CPU wants the *top* address.
        let top = VirtAddr::from_ptr(core::ptr::addr_of!(DF_STACK)) + DF_STACK_SIZE as u64;
        (*core::ptr::addr_of_mut!(TSS)).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            top;
    }
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}

/// `(code, data)` selectors for ring 3 (RPL bits included: 0x2b / 0x23-style).
pub fn user_selectors() -> (u16, u16) {
    (GDT.1.user_code.0, GDT.1.user_data.0)
}

/// Address of the TSS `rsp0` field: the stack the CPU switches to when an
/// interrupt or exception arrives while running ring 3 code.
pub fn rsp0_slot() -> *mut u64 {
    unsafe { core::ptr::addr_of_mut!((*core::ptr::addr_of_mut!(TSS)).privilege_stack_table[0]) as *mut u64 }
}

/// `(kernel code, kernel data)` selectors, for the STAR MSR.
pub fn kernel_selectors() -> (u16, u16) {
    (GDT.1.code_selector.0, GDT.1.data_selector.0)
}
