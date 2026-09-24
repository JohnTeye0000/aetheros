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

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

const MAX_LINE: usize = 128;
const HISTORY_MAX: usize = 32;
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

/// The last few commands the user entered (most recent last). Heap-backed, so
/// it grows as needed; the `history` command lists it. `Vec::new()` is a const
/// fn, so this needs no lazy initialisation.
static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

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

    // Remember non-empty commands so `history` can list them. Keep only the
    // most recent HISTORY_MAX so it can't grow without bound.
    if !input.is_empty() {
        let mut hist = HISTORY.lock();
        hist.push(String::from(input));
        if hist.len() > HISTORY_MAX {
            hist.remove(0);
        }
    }

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
        "help" => {
            out("commands:");
            out("  help  echo  about  ticks  date  mem  ps  spawn  kill  run  save  load  heaptest  cpuinfo  lspci  acpi  hpet  disk  clear  history  crash  pagefault");
            out("  ls  cat <f>  write <f> <txt>  rm <f>  cp <s> <d>  mv <s> <d>");
        }
        "echo" => out(args),
        "about" => out("AetherOS - a from-scratch x86-64 kernel in Rust."),
        "ticks" => {
            let t = crate::interrupts::ticks();
            crate::println!("uptime: {} timer ticks (~{} s)", t, t / 18);
            crate::serial_println!("uptime: {} timer ticks", t);
        }
        "date" => {
            let t = crate::rtc::now();
            crate::println!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                t.year, t.month, t.day, t.hour, t.minute, t.second
            );
            crate::serial_println!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                t.year, t.month, t.day, t.hour, t.minute, t.second
            );
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
        "ps" | "threads" => {
            // One row per thread. `progress` is a per-thread work counter: it
            // climbs for a CPU-hogging thread, and ticks once a second for a
            // sleeping ticker; state shows how many ticks a sleeper has left.
            out("  id  name         state         switches  progress");
            for t in crate::scheduler::snapshot() {
                let state = if t.wake_in > 0 {
                    alloc::format!("sleeping({})", t.wake_in)
                } else {
                    alloc::string::String::from(t.state.name())
                };
                let progress = if t.id == 0 {
                    alloc::string::String::from("-")
                } else {
                    alloc::format!("{}", crate::scheduler::progress(t.id))
                };
                let line = alloc::format!(
                    "{:>4}  {:<11}  {:<12}  {:>8}  {}",
                    t.id, t.name, state, t.switches, progress
                );
                out(&line);
            }
        }
        "spawn" => match args {
            "spin" => {
                let id = crate::scheduler::spawn("spin", crate::scheduler::spin_entry, 0);
                let line = alloc::format!("spawned thread {} (spin: burns CPU, never yields)", id);
                out(&line);
            }
            "ticker" => {
                let id = crate::scheduler::spawn("ticker", crate::scheduler::ticker_entry, 0);
                let line = alloc::format!("spawned thread {} (ticker: sleeps ~1s, wakes, logs)", id);
                out(&line);
            }
            _ => out("usage: spawn spin|ticker"),
        },
        "kill" => match args.parse::<usize>() {
            Ok(id) => match crate::scheduler::kill(id) {
                Ok(()) => {
                    let line = alloc::format!("killed thread {}", id);
                    out(&line);
                }
                Err(crate::scheduler::KillError::IsBootThread) => {
                    out("kill: refusing to kill thread 0 (kernel_main)")
                }
                Err(crate::scheduler::KillError::NoSuchThread) => out("kill: no such thread"),
                Err(crate::scheduler::KillError::AlreadyDead) => out("kill: already dead"),
            },
            Err(_) => out("usage: kill <thread id>"),
        },
        "heaptest" => {
            // Allocate, free, then allocate again at the same size. A bump
            // allocator (Stage 5's original design) could only reclaim space
            // once EVERY allocation was freed at once; this free-list
            // allocator reclaims individual blocks, so the second allocation
            // should land at exactly the address the first one just vacated.
            use alloc::boxed::Box;
            let a = Box::new(0xAAu8);
            let addr_a = &*a as *const u8 as usize;
            drop(a); // returned to the free list immediately

            let b = Box::new(0xBBu8);
            let addr_b = &*b as *const u8 as usize;

            let line1 = alloc::format!("allocated at {:#x}, then freed it", addr_a);
            let line2 = alloc::format!("allocated again (same size) at {:#x}", addr_b);
            let line3 = if addr_a == addr_b {
                alloc::string::String::from(
                    "same address, reused - free-list allocator reclaims individual blocks",
                )
            } else {
                alloc::string::String::from("different address (unexpected for this allocator)")
            };
            for line in [&line1, &line2, &line3] {
                crate::println!("{}", line);
                crate::serial_println!("{}", line);
            }
        }
        "cpuinfo" => {
            let info = crate::cpu::identify();
            let vendor = core::str::from_utf8(&info.vendor).unwrap_or("?");
            crate::println!("vendor: {}", vendor);
            crate::serial_println!("vendor: {}", vendor);

            if info.has_brand {
                // The brand string is padded with trailing NULs; trim them.
                let raw = core::str::from_utf8(&info.brand).unwrap_or("?");
                let brand = raw.trim_end_matches('\0').trim();
                crate::println!("brand:  {}", brand);
                crate::serial_println!("brand:  {}", brand);
            }

            crate::println!(
                "family: {}  model: {}  stepping: {}",
                info.family, info.model, info.stepping
            );
            crate::serial_println!(
                "family: {}  model: {}  stepping: {}",
                info.family, info.model, info.stepping
            );

            crate::print!("features:");
            crate::serial_print!("features:");
            for &(is_edx, bit, name) in crate::cpu::FEATURES {
                if info.has(is_edx, bit) {
                    crate::print!(" {}", name);
                    crate::serial_print!(" {}", name);
                }
            }
            crate::println!();
            crate::serial_println!();
        }
        "lspci" => {
            let devices = crate::pci::scan();
            for d in &devices {
                let class = crate::pci::class_name(d.class);
                let line = match crate::pci::known_name(d.vendor_id, d.device_id) {
                    Some(name) => alloc::format!(
                        "{:02x}:{:02x}.{} {:04x}:{:04x}  {} ({})",
                        d.bus, d.device, d.function, d.vendor_id, d.device_id, name, class
                    ),
                    None => alloc::format!(
                        "{:02x}:{:02x}.{} {:04x}:{:04x}  ({})",
                        d.bus, d.device, d.function, d.vendor_id, d.device_id, class
                    ),
                };
                crate::println!("{}", line);
                crate::serial_println!("{}", line);
            }
            let summary = alloc::format!("{} device(s) on bus 0", devices.len());
            crate::println!("{}", summary);
            crate::serial_println!("{}", summary);
        }
        "acpi" => match crate::acpi::find_rsdp() {
            Some(r) => {
                let oem = core::str::from_utf8(&r.oem_id).unwrap_or("?").trim_end();
                let version = if r.revision >= 2 { "2.0+ (has XSDT)" } else { "1.0 (RSDT only)" };
                let line1 = alloc::format!("RSDP found at physical {:#x}", r.physical_address);
                let line2 = alloc::format!("OEM: {}   ACPI version: {}", oem, version);
                let line3 = if r.revision >= 2 {
                    alloc::format!("RSDT: {:#x}   XSDT: {:#x}", r.rsdt_address, r.xsdt_address)
                } else {
                    alloc::format!("RSDT: {:#x}", r.rsdt_address)
                };
                crate::println!("{}", line1);
                crate::println!("{}", line2);
                crate::println!("{}", line3);
                crate::serial_println!("{}", line1);
                crate::serial_println!("{}", line2);
                crate::serial_println!("{}", line3);

                // Walk the RSDT/XSDT the RSDP points to and list every table
                // it names, checksum-verifying each one along the way.
                let tables = crate::acpi::list_tables(&r);
                for t in &tables {
                    let sig = core::str::from_utf8(&t.signature).unwrap_or("????");
                    let status = if t.checksum_ok { "ok" } else { "BAD CHECKSUM" };
                    let line = alloc::format!(
                        "  {}  {:5} bytes  @ {:#010x}  [{}]",
                        sig, t.length, t.physical_address, status
                    );
                    crate::println!("{}", line);
                    crate::serial_println!("{}", line);
                }

                // If we found the MADT (signature "APIC"), decode its entries
                // to answer the question a future multi-core kernel needs
                // first: how many CPU cores does this machine actually have?
                if let Some(madt_table) = tables.iter().find(|t| &t.signature == b"APIC") {
                    if let Some(madt) = crate::acpi::parse_madt(madt_table) {
                        // Build "0, 1, 2" by hand rather than Vec::join, which
                        // needs an extra trait import (`alloc::slice::Join`)
                        // that's part of std's prelude but not core's/alloc's.
                        let mut ids = alloc::string::String::new();
                        for (i, id) in madt.cpu_apic_ids.iter().enumerate() {
                            if i > 0 {
                                ids.push_str(", ");
                            }
                            ids.push_str(&alloc::format!("{}", id));
                        }
                        let cpu_line = alloc::format!(
                            "MADT: {} CPU core(s) (APIC ids: {})",
                            madt.cpu_apic_ids.len(),
                            ids
                        );
                        crate::println!("{}", cpu_line);
                        crate::serial_println!("{}", cpu_line);

                        for (id, addr, gsi) in &madt.io_apics {
                            let io_line = alloc::format!(
                                "MADT: I/O APIC id {} @ {:#x} (GSI base {})",
                                id, addr, gsi
                            );
                            crate::println!("{}", io_line);
                            crate::serial_println!("{}", io_line);
                        }
                    }
                }
            }
            None => out("no valid RSDP found (checksum failed or not present)"),
        },
        "hpet" => {
            // Same "find the ACPI table by signature, then decode its
            // contents" shape as the MADT handling in `acpi` above — but this
            // table's contents point at a real memory-mapped device instead
            // of describing more ACPI structures.
            let rsdp = crate::acpi::find_rsdp();
            let table = rsdp
                .as_ref()
                .and_then(|r| crate::acpi::list_tables(r).into_iter().find(|t| &t.signature == b"HPET"));
            match table.and_then(|t| crate::hpet::read(&t)) {
                Some(h) => {
                    let line1 = alloc::format!(
                        "HPET base {:#x}  {}-bit counter  {} comparator(s)",
                        h.base_address,
                        if h.counter_is_64bit { 64 } else { 32 },
                        h.num_timers
                    );
                    // Integer-only MHz (whole.fractional), not floating point —
                    // this kernel deliberately runs without SSE/hardware float
                    // (see gdt.rs/target config), so plain u64 math is not just
                    // simpler here, it's the appropriate tool.
                    let freq_hz: u64 = 1_000_000_000_000_000u64 / h.period_femtoseconds as u64;
                    let line2 = alloc::format!(
                        "tick period: {} fs ({}.{:03} MHz)",
                        h.period_femtoseconds,
                        freq_hz / 1_000_000,
                        (freq_hz % 1_000_000) / 1000
                    );
                    let line3 = if h.we_enabled_it {
                        alloc::string::String::from("counter was disabled - enabled it ourselves")
                    } else {
                        alloc::string::String::from("counter was already running")
                    };
                    let line4 = alloc::format!(
                        "counter: {} -> {}  ({} ns elapsed)",
                        h.counter_before, h.counter_after, h.elapsed_ns()
                    );
                    for line in [&line1, &line2, &line3, &line4] {
                        crate::println!("{}", line);
                        crate::serial_println!("{}", line);
                    }
                }
                None => out("no HPET found (table missing, or not signed \"HPET\")"),
            }
        }
        "run" => {
            let arg = args.trim();
            if arg.is_empty() {
                out("usage: run <program>   programs (they run in ring 3):");
                for (name, desc) in crate::usermode::PROGRAMS {
                    let line = alloc::format!("  {:<7} {}", name, desc);
                    out(&line);
                }
            } else {
                match crate::usermode::spawn_builtin(arg) {
                    Ok(id) => {
                        let line = alloc::format!("started '{}' as thread {} (user mode)", arg, id);
                        out(&line);
                    }
                    Err(e) => {
                        let line = alloc::format!("run: {}", e);
                        out(&line);
                    }
                }
            }
        }
        "save" => match crate::persist::save() {
            Ok((files, sectors)) => {
                let line = alloc::format!("saved {} files ({} sectors) to the data disk", files, sectors);
                out(&line);
            }
            Err(e) => {
                let line = alloc::format!("save failed: {}", e);
                out(&line);
            }
        },
        "load" => match crate::persist::load() {
            Ok(n) => {
                let line = alloc::format!("loaded {} files from the data disk", n);
                out(&line);
            }
            Err(e) => {
                let line = alloc::format!("load failed: {}", e);
                out(&line);
            }
        },
        "disk" => match crate::ata::identify(crate::ata::Drive::Master) {
            Some(id) => {
                let model = core::str::from_utf8(&id.model).unwrap_or("?").trim();
                let bytes = id.sectors_28bit as u64 * 512;
                let line1 = alloc::format!("primary master: \"{}\"", model);
                // This kernel's own boot image is well under 1 MiB, so report
                // KiB when the disk is small rather than rounding a genuine
                // ~700 KiB down to a misleading "0 MiB".
                let line2 = if bytes >= 1024 * 1024 {
                    alloc::format!(
                        "{} sectors ({} MiB, 28-bit LBA)",
                        id.sectors_28bit, bytes / (1024 * 1024)
                    )
                } else {
                    alloc::format!(
                        "{} sectors ({} KiB, 28-bit LBA)",
                        id.sectors_28bit, bytes / 1024
                    )
                };
                crate::println!("{}", line1);
                crate::println!("{}", line2);
                crate::serial_println!("{}", line1);
                crate::serial_println!("{}", line2);

                // Read sector 0 for real (READ SECTORS, no writes anywhere in
                // this driver) and check the boot signature every MBR-style
                // sector ends with — proof the bytes we got back are genuine.
                match crate::ata::read_sector(crate::ata::Drive::Master, 0) {
                    Some(sector) => {
                        let sig_ok = sector[510] == 0x55 && sector[511] == 0xAA;
                        let line3 = alloc::format!(
                            "sector 0: last bytes {:#04x} {:#04x}  boot signature: {}",
                            sector[510], sector[511],
                            if sig_ok { "valid (0x55AA)" } else { "unexpected" }
                        );
                        crate::println!("{}", line3);
                        crate::serial_println!("{}", line3);
                    }
                    None => out("sector 0 read failed"),
                }
            }
            None => out("no ATA drive found on the primary bus"),
        },
        "clear" => crate::vga_buffer::clear(),
        "crash" => {
            out("deliberately overflowing the kernel stack...");
            out("the double-fault handler should catch it and halt (no reboot).");
            stack_overflow();
        }
        "pagefault" => {
            out("writing to an unmapped address (0xdeadbeef)...");
            out("the page-fault handler should report it and halt.");
            let ptr = 0xdead_beef as *mut u8;
            unsafe { *ptr = 42 };
        }
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
        "cp" => match args.split_once(' ') {
            // cp <src> <dst> — copy a file's contents to a new name
            Some((src, dst)) => match crate::fs::read(src) {
                Some(contents) => {
                    crate::fs::write(dst, &contents);
                    crate::println!("copied {} -> {}", src, dst);
                    crate::serial_println!("copied {} -> {}", src, dst);
                }
                None => {
                    crate::println!("cp: {}: no such file", src);
                    crate::serial_println!("cp: {}: no such file", src);
                }
            },
            None => out("usage: cp <src> <dst>"),
        },
        "mv" => match args.split_once(' ') {
            // mv <src> <dst> — copy then delete the original (a rename)
            Some((src, dst)) => match crate::fs::read(src) {
                Some(contents) => {
                    crate::fs::write(dst, &contents);
                    crate::fs::remove(src);
                    crate::println!("moved {} -> {}", src, dst);
                    crate::serial_println!("moved {} -> {}", src, dst);
                }
                None => {
                    crate::println!("mv: {}: no such file", src);
                    crate::serial_println!("mv: {}: no such file", src);
                }
            },
            None => out("usage: mv <src> <dst>"),
        },
        "history" => {
            for (i, cmd) in HISTORY.lock().iter().enumerate() {
                crate::println!("{:>3}  {}", i + 1, cmd);
                crate::serial_println!("{:>3}  {}", i + 1, cmd);
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

/// Recurse forever to overflow the kernel stack — a deliberate crash that shows
/// the double-fault handler (running on its own IST stack, see gdt.rs) catching
/// what would otherwise be a triple-fault reboot. The volatile read after the
/// recursive call stops the compiler from turning this into a plain loop.
#[allow(unconditional_recursion)]
fn stack_overflow() {
    let x = 0u8;
    stack_overflow();
    unsafe { core::ptr::read_volatile(&x) };
}
