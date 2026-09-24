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

use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

// --- Hardware interrupts (8259 PIC) -------------------------------------
//
// A PC has two chained 8259 PIC chips that funnel hardware interrupt lines
// (IRQs) from devices — timer, keyboard, disk — into the CPU. By default they
// deliver IRQs on vectors 0..15, but the CPU already reserves vectors 0..31 for
// exceptions (vector 8 = double fault!). So step one is to *remap* the PICs to
// land just above the exceptions, at 32..47.
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8; // 40

/// The two cascaded PICs, behind a spinlock. `ChainedPics::new` is `unsafe`
/// because wrong offsets would misroute interrupts; ours are correct (32/40).
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Which interrupt vector each device's IRQ is remapped to. IRQ0 (the timer)
/// becomes vector 32; IRQ1 (the keyboard, next milestone) will be 33.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,        // IRQ0 -> vector 32
    Keyboard = PIC_1_OFFSET + 1, // IRQ1 -> vector 33
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

/// How many timer interrupts have fired since boot. `AtomicU64` so the handler
/// can update it without a lock (a plain `static mut` would be `unsafe`).
static TICKS: AtomicU64 = AtomicU64::new(0);

lazy_static! {
    /// The single IDT for the whole kernel. The CPU only stores a *pointer* to
    /// this table (set by `lidt`), so the table itself must live for the entire
    /// run — hence `static`. `lazy_static` lets us build it with normal code at
    /// first use instead of hand-writing a `const` table of raw descriptors.
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        // Vector 3: breakpoint. Vector 8: double fault.
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        // Run the double-fault handler on its own IST stack (see gdt.rs) so a
        // stack overflow can't escalate it into a triple-fault reboot.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        // Vector 14: page faults — report the faulting address instead of
        // dying silently (an unhandled page fault would escalate to a double
        // fault).
        idt.page_fault.set_handler_fn(page_fault_handler);
        // Faults a user program can trigger. From ring 3 they kill just that
        // program; from ring 0 they are kernel bugs and panic.
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_handler);
        // Vector 0x80: the system-call gate. DPL 3 is what lets ring 3 code
        // execute `int 0x80` at all (other vectors raise #GP for user code).
        unsafe {
            idt[0x80]
                .set_handler_addr(x86_64::VirtAddr::new(crate::usermode::syscall_entry_addr()))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        // Vector 32: the timer IRQ (IRQ0), now that the PIC is remapped there.
        idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        // Vector 33: the keyboard IRQ (IRQ1).
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt
    };

    /// The keyboard decoder, behind a spinlock. A PS/2 keyboard sends raw
    /// "scancodes" (one number per key press/release, in "set 1" encoding); this
    /// state machine turns the stream of scancodes into actual characters,
    /// tracking shift/caps and applying a US layout.
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> = Mutex::new(
        Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::Ignore)
    );
}

/// Load our IDT into the CPU with the `lidt` instruction. After this returns,
/// exceptions are dispatched to the handlers below instead of escalating.
pub fn init() {
    IDT.load();
}

/// Bring the hardware interrupts online: remap + initialise the PICs, then run
/// `sti` to actually let the CPU accept interrupts. After this, the timer chip
/// starts firing IRQ0 at us ~18.2 times a second with no further help from us.
///
/// `PICS.initialize()` is `unsafe` because talking to the wrong I/O ports could
/// wedge the interrupt controller — but our offsets are the correct 32/40.
pub fn init_hardware() {
    unsafe { PICS.lock().initialize() };

    // Set the PIC interrupt masks explicitly. `initialize()` restores whatever
    // mask the BIOS left behind, which can leave the keyboard line disabled.
    // On the PIC's mask register a 0 bit ENABLES that IRQ. We enable IRQ0
    // (timer), IRQ1 (keyboard), and IRQ2 (the cascade line to the second PIC),
    // and mask everything else for now.
    use x86_64::instructions::port::Port;
    let mut pic1_mask: Port<u8> = Port::new(0x21); // primary PIC data port
    let mut pic2_mask: Port<u8> = Port::new(0xA1); // secondary PIC data port
    unsafe {
        pic1_mask.write(0b1111_1000); // enable IRQ0, IRQ1, IRQ2
        pic2_mask.write(0b1111_1111); // secondary PIC fully masked (unused yet)
    }

    // `sti` — from here the CPU will actually accept the interrupts we enabled.
    x86_64::instructions::interrupts::enable();
}

/// How many timer ticks have elapsed since boot (a rough uptime: ~18.2/sec).
pub fn ticks() -> u64 {
    TICKS.load(Ordering::SeqCst)
}

/// Timer interrupt (IRQ0 → vector 32). Fired by the PIT roughly 18.2 times per
/// second. We print the first few ticks as proof the hardware is driving us,
/// then go quiet so we don't flood the log.
///
/// The non-negotiable last step is `notify_end_of_interrupt`: the PIC waits for
/// this "I'm done" signal before it will deliver the *next* interrupt on that
/// line. Forget it and the timer fires exactly once, then silence forever.
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if tick <= 5 {
        crate::println!("[ irq  ] timer tick #{} (hardware woke the CPU)", tick);
        crate::serial_println!("[ irq  ] timer interrupt fired - tick #{}", tick);
    }
    // EOI *before* the context switch: if we switch to a brand-new thread it
    // starts at `thread_start`, not by returning through this handler, so an
    // EOI left for later would never be sent and the PIC would stop delivering
    // timer interrupts entirely.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
    // Preemption: hand the CPU to the next ready thread, if there is one.
    crate::scheduler::on_tick();
}

/// Keyboard interrupt (IRQ1 → vector 33). Fires once per key press AND once per
/// release. The PS/2 controller holds the raw scancode in I/O port 0x60, and it
/// will NOT send another keyboard interrupt until we read that byte — so reading
/// the port is mandatory, not optional.
///
/// We feed the scancode to the `pc-keyboard` decoder, which assembles it into a
/// real character (handling multi-byte sequences, shift, caps lock, etc.), then
/// echo that character to both the screen and the serial log.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use pc_keyboard::DecodedKey;
    use x86_64::instructions::port::Port;

    let mut keyboard = KEYBOARD.lock();
    let mut port: Port<u8> = Port::new(0x60); // PS/2 data port
    let scancode: u8 = unsafe { port.read() };

    // add_byte feeds one scancode into the state machine; it yields a key event
    // only once a full key is assembled. process_keyevent then maps that event
    // (plus modifier state) to an actual character.
    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(key_event) {
            // Feed printable keys to the shell; ignore raw/special keys for now.
            if let DecodedKey::Unicode(character) = key {
                crate::shell::on_char(character);
            }
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
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

/// Page fault (`#PF`, vector 14) — the CPU couldn't translate a virtual address
/// (it's unmapped, or the access violated the page's permissions). This is the
/// single most common memory bug, so a good handler is worth its weight.
///
/// Two pieces of information make it debuggable: the faulting address, which the
/// CPU leaves in the `CR2` register, and an `error_code` describing the access
/// (was it a read or write? user or kernel? a protection violation or a missing
/// page?). We report both. We can't fix the fault, so we stop rather than return
/// (returning would just re-run the faulting instruction and fault again).
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    // Came from ring 3? Then it's the program's fault, not the kernel's: kill
    // the program and carry on.
    if stack_frame.code_segment & 3 == 3 {
        crate::usermode::user_fault(
            1,
            Cr2::read().as_u64(),
            stack_frame.instruction_pointer.as_u64(),
        );
    }

    crate::vga_buffer::WRITER
        .lock()
        .set_color(crate::vga_buffer::Color::White, crate::vga_buffer::Color::Red);
    crate::println!("*** PAGE FAULT ***");
    crate::println!("accessed address: {:?}", Cr2::read());
    crate::println!("error code:       {:?}", error_code);
    crate::println!("{:#?}", stack_frame);
    crate::serial_println!(
        "*** PAGE FAULT at {:?} (error {:?})",
        Cr2::read(),
        error_code
    );

    // Unrecoverable here — park the CPU.
    loop {
        x86_64::instructions::hlt();
    }
}

/// True if the exception was raised while running user (ring 3) code.
fn from_user(frame: &InterruptStackFrame) -> bool {
    frame.code_segment & 3 == 3
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    if from_user(&stack_frame) {
        crate::usermode::user_fault(4, 0, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: DIVIDE ERROR
{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    if from_user(&stack_frame) {
        crate::usermode::user_fault(3, 0, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: INVALID OPCODE
{:#?}", stack_frame);
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_user(&stack_frame) {
        crate::usermode::user_fault(2, error_code, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: GENERAL PROTECTION FAULT (error {:#x})
{:#?}", error_code, stack_frame);
}
