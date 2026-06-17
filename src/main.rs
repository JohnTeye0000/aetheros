//! AetherOS kernel — entry point.
//!
//! This is a *freestanding* binary: no standard library, no operating system
//! underneath us, no `main`. The bootloader hands control to `_start`, and at
//! that moment the CPU, the screen, and all of memory are ours to manage.

#![no_std] // don't link the Rust standard library — there's no OS to provide it
#![no_main] // we define our own entry point, not the usual runtime `main`
#![feature(abi_x86_interrupt)] // unstable ABI needed to write CPU exception handlers

use core::panic::PanicInfo;

mod interrupts;
mod serial;
mod vga_buffer;
use vga_buffer::Color;

/// Log a line to BOTH the screen (VGA) and the serial port (host terminal).
/// The screen is the user-facing console; serial is the developer's window in.
macro_rules! boot_log {
    ($($arg:tt)*) => {{
        println!($($arg)*);
        serial_println!($($arg)*);
    }};
}

/// The kernel entry point. The bootloader jumps here.
///
/// `extern "C"` + `#[no_mangle]` keep the name exactly `_start` so the linker
/// can find it. It never returns (`!`) — there is nothing to return *to*.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightCyan, Color::Black);
    boot_log!("AetherOS  //  bare-metal x86-64 kernel");

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGray, Color::Black);
    boot_log!("");
    boot_log!("[ boot ] handed control by the bootloader at _start");
    boot_log!("[ boot ] no OS beneath us - the machine is ours");
    boot_log!("[ vga  ] writing directly to text buffer @ 0xb8000");
    boot_log!("[ uart ] mirroring kernel log to serial COM1");
    boot_log!("");

    // --- Stage 1: CPU exception handling (IDT) --------------------------
    // Install handlers so the CPU can recover from exceptions instead of
    // triple-faulting, then prove it by deliberately raising a breakpoint.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::Yellow, Color::Black);
    boot_log!("");
    boot_log!("[ idt  ] loading the interrupt descriptor table");
    interrupts::init();
    boot_log!("[ idt  ] raising a breakpoint exception (int3) on purpose...");
    x86_64::instructions::interrupts::int3();
    boot_log!("[ idt  ] ...and we are back - the CPU recovered cleanly");

    // --- Stage 2: hardware interrupts (PIC + timer) ---------------------
    // Bring the 8259 PIC online and enable interrupts. From here on, the timer
    // chip fires at the CPU on its own ~18 times a second — execution is now
    // driven by hardware, not just our straight-line code.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightCyan, Color::Black);
    boot_log!("");
    boot_log!("[ pic  ] remapping the 8259 PIC and enabling hardware interrupts");
    interrupts::init_hardware();
    boot_log!("[ pic  ] interrupts live - the timer is now driving the CPU:");
    boot_log!("");

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);

    // Idle in a halt loop: `hlt` stops the core until the NEXT interrupt
    // arrives, so we sleep between timer ticks instead of burning 100% CPU.
    // This is how a real kernel waits for work.
    loop {
        x86_64::instructions::hlt();
    }
}

/// Called by the compiler when anything panics. In a kernel there is no one
/// above us to catch it, so we print the reason — to screen and serial — and
/// stop.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    vga_buffer::WRITER.lock().set_color(Color::White, Color::Red);
    println!("*** KERNEL PANIC ***");
    println!("{}", info);
    serial_println!("*** KERNEL PANIC: {}", info);
    loop {}
}
