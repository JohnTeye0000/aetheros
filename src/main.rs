//! AetherOS kernel — entry point.
//!
//! This is a *freestanding* binary: no standard library, no operating system
//! underneath us, no `main`. The bootloader hands control to `_start`, and at
//! that moment the CPU, the screen, and all of memory are ours to manage.

#![no_std] // don't link the Rust standard library — there's no OS to provide it
#![no_main] // we define our own entry point, not the usual runtime `main`
#![feature(abi_x86_interrupt)] // unstable ABI needed to write CPU exception handlers

// Pull in the `alloc` crate — the slice of the standard library that provides
// heap types (Box, Vec, String). It works under no_std once we supply a global
// allocator, which the `allocator` module does.
extern crate alloc;

use core::panic::PanicInfo;

use bootloader::{entry_point, BootInfo};

mod acpi;
mod allocator;
mod ata;
mod cpu;
mod fs;
mod gdt;
mod hpet;
mod interrupts;
mod memory;
mod pci;
mod rtc;
mod scheduler;
mod serial;
mod shell;
mod task;
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

// The bootloader calls our entry point. `entry_point!` generates the real
// `_start` symbol for us AND type-checks the signature, so we safely receive a
// `&BootInfo` (memory map + physical-memory offset) instead of digging it out
// of a register by hand. It never returns (`!`) — there is nothing to return to.
entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
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
    boot_log!("[ gdt  ] loading GDT + TSS (double-fault stack via IST)");
    gdt::init();
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
    boot_log!("[ kbd  ] PS/2 keyboard armed - type and the kernel echoes you back");
    boot_log!("");

    boot_log!("Stage 3 live. Timer + keyboard interrupts active.");
    boot_log!("");

    // --- Stage 4: paging & physical memory ------------------------------
    // Read the page tables the bootloader handed us, then author our own
    // mapping to prove the kernel controls virtual memory.
    use x86_64::structures::paging::{Page, Translate};
    use x86_64::VirtAddr;

    vga_buffer::WRITER
        .lock()
        .set_color(Color::Yellow, Color::Black);
    boot_log!("[ mem  ] taking ownership of the page tables");

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };
    // Tally total usable RAM now, while we still have the boot info (the shell's
    // `mem` command reads it back later).
    memory::record_usable_ram(&boot_info.memory_map);

    // Translate a few virtual addresses to their physical frames — proof we can
    // read the tables. (VGA is identity-mapped; the offset maps to physical 0;
    // the stack lives wherever the bootloader placed it.)
    let stack_probe = 0u64;
    let probes = [
        ("vga buffer ", 0xb8000u64),
        ("phys offset", boot_info.physical_memory_offset),
        ("stack var  ", &stack_probe as *const _ as u64),
    ];
    for (label, addr) in probes {
        let phys = mapper.translate_addr(VirtAddr::new(addr));
        boot_log!("[ mem  ] {} {:#014x} -> {:?}", label, addr, phys);
    }

    // Now WRITE the tables: map an unused virtual page onto the VGA frame and
    // write "New!" through it — text reaches the screen via a mapping we built.
    boot_log!("[ mem  ] mapping a fresh page -> VGA frame, writing through it");
    let page = Page::containing_address(VirtAddr::new(0xdead_beef_000));
    memory::create_example_mapping(page, &mut mapper, &mut frame_allocator);
    let page_ptr: *mut u64 = page.start_address().as_mut_ptr();
    unsafe { page_ptr.offset(400).write_volatile(0x_f021_f077_f065_f04e) };
    boot_log!("[ mem  ] 'New!' now on screen via page 0xdeadbeef000");
    boot_log!("");

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);
    boot_log!("Stage 4 live. Kernel owns its page tables.");
    boot_log!("");

    // --- Stage 5: a kernel heap (dynamic allocation) --------------------
    // Map a region of pages and arm the global allocator. After this, the heap
    // types from `alloc` — Box, Vec, String — work inside the kernel.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::Yellow, Color::Black);
    boot_log!(
        "[ heap ] mapping a {} KiB heap and arming the allocator",
        allocator::HEAP_SIZE / 1024
    );
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap init failed");

    {
        use alloc::{boxed::Box, string::String, vec::Vec};

        let boxed = Box::new(42u64);
        boot_log!("[ heap ] Box<u64> at {:p} holds {}", boxed, *boxed);

        let mut v: Vec<u64> = Vec::new();
        for i in 1..=100 {
            v.push(i);
        }
        let sum: u64 = v.iter().sum();
        boot_log!("[ heap ] Vec of {} numbers, sum(1..=100) = {}", v.len(), sum);

        let s = String::from("a heap-allocated string");
        boot_log!("[ heap ] String: \"{}\" ({} bytes)", s, s.len());
    }
    boot_log!("");

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);
    boot_log!("Stage 5 live. Heap online - Box/Vec/String work.");
    boot_log!("");

    // --- Stage 6: cooperative multitasking (async/await) ----------------
    // Spawn two async tasks on a tiny round-robin executor. Each yields after
    // every step, so the executor interleaves them: two things making progress
    // "at once" on one CPU, cooperatively.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::Yellow, Color::Black);
    boot_log!("[ task ] spawning two async tasks on the cooperative executor");
    {
        use task::{executor::Executor, Task};
        let mut executor = Executor::new();
        executor.spawn(Task::new(count_task("A", 3)));
        executor.spawn(Task::new(count_task("B", 3)));
        executor.run(); // runs until both tasks finish; note how A and B interleave
    }
    boot_log!("[ task ] both tasks finished - executor drained");
    boot_log!("");

    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);
    boot_log!("Stage 6 live. Cooperative multitasking works.");
    boot_log!("");

    // --- Stages 23-24: preemptive threads ---------------------------------
    // Adopt this code as thread 0. From here on the timer interrupt can swap
    // threads at any moment; the shell's `spawn`/`ps`/`kill` create, list and
    // remove threads on demand.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::Yellow, Color::Black);
    boot_log!("[ sched ] adopting kernel_main as thread 0");
    scheduler::init();
    boot_log!("[ sched ] scheduler online - try: spawn ticker / spawn spin / ps / kill <id>");
    boot_log!("");
    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightGreen, Color::Black);
    boot_log!("Stage 24 live. Preemptive threads with spawn / sleep / kill.");
    boot_log!("");

    // --- Stage 7: an interactive shell ----------------------------------
    // Hand the keyboard over to a command shell. From here the kernel is
    // event-driven: it sleeps in `hlt` and wakes on each keystroke, which the
    // keyboard interrupt feeds into `shell::on_char`. Type a command, press
    // Enter, and the kernel runs it.
    vga_buffer::WRITER
        .lock()
        .set_color(Color::LightCyan, Color::Black);
    boot_log!("Stage 7 live. Interactive shell ready.");
    boot_log!("");
    vga_buffer::WRITER
        .lock()
        .set_color(Color::White, Color::Black);
    shell::start();

    // Idle in a halt loop: `hlt` stops the core until the next interrupt
    // (a keystroke or timer tick), so we sleep instead of burning 100% CPU.
    // The shell itself runs from inside the keyboard interrupt handler.
    loop {
        x86_64::instructions::hlt();
    }
}

/// A demo async task: print `n` steps, yielding the CPU after each one so the
/// executor can let other tasks run in between. Two of these spawned together
/// will interleave (A1, B1, A2, B2, ...), which is the whole point.
async fn count_task(name: &'static str, n: u32) {
    for i in 1..=n {
        boot_log!("[ task ] task {} : step {}/{}", name, i, n);
        task::yield_now().await;
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
