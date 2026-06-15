//! AetherOS kernel — entry point.
//!
//! This is a *freestanding* binary: no standard library, no operating system
//! underneath us, no `main`. The bootloader hands control to `_start`, and at
//! that moment the CPU, the screen, and all of memory are ours to manage.

#![no_std] // don't link the Rust standard library — there's no OS to provide it
#![no_main] // we define our own entry point, not the usual runtime `main`

use core::panic::PanicInfo;

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

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);
    boot_log!("Stage 0 complete. Kernel is alive. Halting.");

    // Nothing to do yet — park the CPU. (A real `hlt` loop arrives with the
    // interrupt milestone; for now we just spin.)
    loop {}
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
