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

lazy_static! {
    /// The TSS, holding one dedicated stack for the double-fault handler.
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5; // 20 KiB is plenty for a handler
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            // Stacks grow downward, so the CPU wants the *top* address.
            let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
            stack_start + STACK_SIZE as u64
        };
        tss
    };
}

/// The segment selectors we need after loading the GDT.
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, tss_selector })
    };
}

/// Load our GDT, point the CPU's code segment at our entry, and load the TSS.
/// Must run before the IDT is set up, since the IDT references the IST index.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}
