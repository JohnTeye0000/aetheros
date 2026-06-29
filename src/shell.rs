//! A tiny in-kernel shell.
//!
//! This is where everything so far comes together into something you can
//! *use*. The keyboard interrupt (Stage 3) feeds characters in here one at a
//! time; we collect them into a line, echo them to the screen, and when you
//! press Enter we parse the line into a command and run it.
//!
//! The line buffer is a fixed-size array, not a heap `String`, on purpose: it's
//! written from inside the keyboard *interrupt handler*, and a fixed array means
//! no allocation can happen at interrupt time (simpler and safer).

use spin::Mutex;

const MAX_LINE: usize = 128;
const PROMPT: &str = "aether> ";

/// The current input line being typed.
struct Line {
    buf: [u8; MAX_LINE],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Line {
            buf: [0; MAX_LINE],
            len: 0,
        }
    }
    fn push(&mut self, byte: u8) -> bool {
        if self.len < MAX_LINE {
            self.buf[self.len] = byte;
            self.len += 1;
            true
        } else {
            false
        }
    }
    fn backspace(&mut self) -> bool {
        if self.len > 0 {
            self.len -= 1;
            true
        } else {
            false
        }
    }
}

static LINE: Mutex<Line> = Mutex::new(Line::new());

/// Print the prompt (to screen and serial).
pub fn prompt() {
    crate::print!("{}", PROMPT);
    crate::serial_print!("{}", PROMPT);
}

/// Show the welcome banner and the first prompt when the shell starts.
pub fn start() {
    out("AetherOS shell. Type 'help' for commands.");
    prompt();
}

/// Feed one decoded character into the shell. Called from the keyboard
/// interrupt handler for every key the user presses.
pub fn on_char(c: char) {
    match c {
        // Enter: finish the line, run it, show a fresh prompt.
        '\n' | '\r' => {
            crate::print!("\n");
            crate::serial_print!("\n");
            run();
            prompt();
        }
        // Backspace / Delete: erase the last character on screen and in the buffer.
        '\u{8}' | '\u{7f}' => {
            let mut line = LINE.lock();
            if line.backspace() {
                crate::print!("\u{8} \u{8}"); // back, overwrite with space, back
            }
        }
        // Printable ASCII: store it and echo it.
        c if is_printable(c) => {
            let mut line = LINE.lock();
            if line.push(c as u8) {
                crate::print!("{}", c);
                crate::serial_print!("{}", c);
            }
        }
        _ => {}
    }
}

fn is_printable(c: char) -> bool {
    (0x20..0x7f).contains(&(c as u32))
}

/// Copy the line out, clear the buffer, then parse and run the command. We copy
/// first so we don't hold the line lock while the command prints its output.
fn run() {
    let mut local = [0u8; MAX_LINE];
    let n = {
        let mut line = LINE.lock();
        let n = line.len;
        local[..n].copy_from_slice(&line.buf[..n]);
        line.len = 0; // clear for the next line
        n
    };

    let input = core::str::from_utf8(&local[..n]).unwrap_or("").trim();
    dispatch(input);
}

/// Parse `input` into a command word + the rest as arguments, and run it.
fn dispatch(input: &str) {
    let (cmd, args) = match input.split_once(' ') {
        Some((c, rest)) => (c, rest.trim()),
        None => (input, ""),
    };

    match cmd {
        "" => {} // empty line — do nothing
        "help" => out("commands: help, echo, about, ticks, mem, clear, ls, cat <f>, write <f> <text>, rm <f>"),
        "echo" => out(args),
        "about" => out("AetherOS - a from-scratch x86-64 kernel in Rust."),
        "ticks" => {
            let t = crate::interrupts::ticks();
            crate::println!("uptime: {} timer ticks (~{} s)", t, t / 18);
            crate::serial_println!("uptime: {} timer ticks", t);
        }
        "mem" => {
            let (used, total, allocs) = crate::allocator::heap_stats();
            let ram = crate::memory::usable_ram();
            crate::println!(
                "heap: {} / {} bytes in use ({} live allocations)",
                used, total, allocs
            );
            crate::println!("RAM:  {} bytes usable (~{} MiB)", ram, ram / (1024 * 1024));
            crate::serial_println!(
                "heap {}/{} bytes, {} allocs; RAM {} bytes",
                used, total, allocs, ram
            );
        }
        "clear" => crate::vga_buffer::clear(),
        "ls" => {
            // size + name for each file in the ramdisk
            for (name, size) in crate::fs::list() {
                crate::println!("{:>5}  {}", size, name);
                crate::serial_println!("{:>5}  {}", size, name);
            }
        }
        "cat" => {
            if args.is_empty() {
                out("usage: cat <file>");
            } else {
                match crate::fs::read(args) {
                    Some(contents) => {
                        crate::print!("{}", contents);
                        crate::serial_print!("{}", contents);
                        // make sure the next prompt starts on a fresh line
                        if !contents.ends_with('\n') {
                            crate::println!();
                            crate::serial_println!();
                        }
                    }
                    None => {
                        crate::println!("cat: {}: no such file", args);
                        crate::serial_println!("cat: {}: no such file", args);
                    }
                }
            }
        }
        "write" => match args.split_once(' ') {
            // write <file> <text> — create or overwrite a file
            Some((name, text)) => {
                crate::fs::write(name, text);
                crate::println!("wrote {} ({} bytes)", name, text.len());
                crate::serial_println!("wrote {} ({} bytes)", name, text.len());
            }
            None if !args.is_empty() => {
                crate::fs::write(args, "");
                crate::println!("wrote {} (empty)", args);
                crate::serial_println!("wrote {} (empty)", args);
            }
            None => out("usage: write <file> <text>"),
        },
        "rm" => {
            if args.is_empty() {
                out("usage: rm <file>");
            } else if crate::fs::remove(args) {
                crate::println!("removed {}", args);
                crate::serial_println!("removed {}", args);
            } else {
                crate::println!("rm: {}: no such file", args);
                crate::serial_println!("rm: {}: no such file", args);
            }
        }
        other => {
            crate::println!("unknown command: '{}' (try 'help')", other);
            crate::serial_println!("unknown command: '{}'", other);
        }
    }
}

/// Print a line to both screen and serial.
fn out(s: &str) {
    crate::println!("{}", s);
    crate::serial_println!("{}", s);
}
