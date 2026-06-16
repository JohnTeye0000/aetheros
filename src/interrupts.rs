//! CPU exception handling via the Interrupt Descriptor Table (IDT).
//!
//! When the CPU hits an exceptional condition — a breakpoint (`int3`), a
//! divide-by-zero, an invalid opcode, a page fault — it stops what it's doing
//! and looks up a handler in the **IDT**, a table the hardware indexes by
//! exception number (vector). If there's no handler for that vector, the fault
//! escalates: a fault *while handling a fault* becomes a **double fault**, and a
//! fault while handling THAT becomes a **triple fault**, which resets the
//! machine. (That triple fault is exactly the reboot loop we debugged on day 1.)
//!
//! So the deal is simple: build an IDT, point at least a few vectors at real
//! handler functions, load it into the CPU, and now the processor can *recover*
//! from an exception instead of rebooting.

use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

lazy_static! {
    /// The single IDT for the whole kernel. The CPU only stores a *pointer* to
    /// this table (set by `lidt`), so the table itself must live for the entire
    /// run — hence `static`. `lazy_static` lets us build it with normal code at
    /// first use instead of hand-writing a `const` table of raw descriptors.
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        // Vector 3: breakpoint. Vector 8: double fault.
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.double_fault.set_handler_fn(double_fault_handler);
        idt
    };
}

/// Load our IDT into the CPU with the `lidt` instruction. After this returns,
/// exceptions are dispatched to the handlers below instead of escalating.
pub fn init() {
    IDT.load();
}

/// Breakpoint exception (`#BP`, vector 3) — raised by the one-byte `int3`
/// instruction. This is the *gentle* exception: the handler runs, and when it
/// returns the CPU resumes at the instruction right after the `int3`, as if
/// nothing happened. Debuggers patch `int3` over an instruction to pause a
/// program exactly here.
///
/// `extern "x86-interrupt"` is a special calling convention: on entry the CPU
/// has pushed an `InterruptStackFrame` (the saved instruction pointer, code
/// segment, flags, stack pointer), and on return the handler must use `iretq`
/// rather than a normal `ret`. The compiler handles all of that for us.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    crate::println!("  -> breakpoint handler ran (we are now inside our own kernel code)");
    crate::serial_println!(
        "[ idt  ] breakpoint (#BP) handled at instruction {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
}

/// Double fault (`#DF`, vector 8) — raised when a fault occurs while the CPU is
/// already trying to deliver an earlier fault (for example, an exception whose
/// vector has no handler). Catching it is what stops a triple-fault reboot.
///
/// Its signature differs from a normal exception in two ways: it receives an
/// `error_code` (always 0 here), and it returns `!` — you can never *resume*
/// from a double fault, only stop. We surface it through the kernel panic path.
///
/// (A production-grade double-fault handler runs on its own known-good stack via
/// the GDT/TSS Interrupt Stack Table, so a stack overflow can't take it down.
/// That hardening is a later milestone; this gets us a clean report today.)
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}
