//! Serial-port (UART 16550) driver — the kernel's debug channel.
//!
//! The screen is for the user; the serial port is for *us*. The 16550 UART is
//! a standard PC chip reachable through I/O port 0x3F8 (COM1). QEMU can pipe
//! that port straight to the host terminal with `-serial stdio`, so anything
//! we send here lands in the developer's console — which is how we verify the
//! kernel booted without needing a screen at all.

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;

lazy_static! {
    /// The first serial port (COM1), behind a spinlock like the VGA writer.
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    // Same interrupt-safety reasoning as the VGA writer: hold the serial lock
    // with interrupts disabled so the timer handler (which also logs to serial)
    // can never deadlock against an in-progress print.
    x86_64::instructions::interrupts::without_interrupts(|| {
        SERIAL1
            .lock()
            .write_fmt(args)
            .expect("printing to serial failed");
    });
}

/// Print to the host via the serial port (no newline).
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

/// Print to the host via the serial port, with a trailing newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}
