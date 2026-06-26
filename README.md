# AetherOS Kernel

A small **x86-64 operating-system kernel written from scratch in Rust** — booted on real boot infrastructure, running on bare metal with no operating system beneath it.

This is a learning-in-the-open project, built one capability at a time. The goal is not to clone Linux; it's to genuinely understand how a computer works from the moment the bootloader hands over control — and to be able to explain every layer.

> Built from the boot sector up, one commit at a time.

---

## What works so far

- [x] **Boots** — a freestanding `#![no_std]` / `#![no_main]` binary; the bootloader jumps to our `_start`.
- [x] **VGA text driver** — writes characters directly to the text buffer at physical `0xb8000`, with scrolling, colour, and `print!` / `println!` macros.
- [x] **Serial driver** — a UART 16550 (COM1) driver with `serial_println!`, mirroring the kernel log to the host so the boot can be verified headlessly.
- [x] **CPU exceptions (IDT)** — an Interrupt Descriptor Table with a breakpoint (`#BP`) handler and a double-fault handler. The kernel raises an `int3` on purpose, handles it, and resumes — the first time it survives an exception instead of triple-faulting.
- [x] **Hardware interrupts (PIC + timer)** — the 8259 PIC is remapped (vectors 32+) and interrupts are enabled; the PIT timer fires IRQ0 ~18×/sec and the kernel handles each tick (with End-Of-Interrupt) while idling in a `hlt` loop. Execution is now driven by hardware. Printing was made interrupt-safe (locks held with interrupts disabled) to avoid deadlock.
- [x] **PS/2 keyboard input** — IRQ1 is unmasked and handled; raw scancodes are read from port `0x60` and decoded (via `pc-keyboard`) into characters, which the kernel echoes to the screen. The first time the kernel reacts to *you*.
- [x] **Paging / virtual memory** — the kernel reads the active page tables (translating any virtual address to its physical frame via the recursive offset map) and **writes its own mappings**: it maps a fresh, unused virtual page onto the VGA frame and prints through it. Plus a bootloader-memory-map frame allocator. The foundation for a heap and, later, separate address spaces.
- [x] **Kernel heap** — a 100 KiB heap region is mapped and managed by a **bump allocator written from scratch** (no external crate), registered as Rust's `#[global_allocator]`. `Box`, `Vec`, and `String` now work inside the kernel.
- [x] **Cooperative multitasking** — an async/await `Task` type and a small round-robin executor (with a hand-built `Waker`). Two async tasks that `yield_now().await` after each step interleave on a single CPU — concurrency without threads or preemption.
- [x] **Interactive shell** — type a command, press Enter, the kernel runs it. The keyboard interrupt feeds a line buffer; built-in commands: `help`, `echo`, `about`, `ticks` (live uptime), `clear`, `ls`, `cat`. **You can drive the OS.**
- [x] **Read-only filesystem (ramdisk)** — a set of named files baked into the kernel image, behind a lookup-by-name interface. `ls` lists them (with sizes), `cat <file>` prints contents. The abstraction a real disk-backed store would later slot behind.
- [x] **Panic handler** — a kernel-level last resort that paints the screen and halts.

### Proof it boots

Booted in QEMU, then driven interactively — listing and reading files in the
kernel's own shell:

![AetherOS shell: ls and cat on the ramdisk filesystem](docs/boot-stage8.png)

The same session over the serial port:

```
AetherOS shell. Type 'help' for commands.
aether> ls
   90  motd
  103  readme.txt
  121  stages.txt
aether> cat motd
Welcome to AetherOS.
An x86-64 operating system built from scratch, one commit at a time.
aether>
```

(Every earlier stage prints its own proof too — exceptions, the timer, paging
(a self-mapped "New!" on screen), the heap, and two async tasks interleaving.
Screenshots: `docs/boot-stage0..6.png`.)

## The build arc (the roadmap)

Each item is a real, independently-committable capability — built in order, roughly the classic OS-dev progression:

- [x] CPU exceptions via an Interrupt Descriptor Table (IDT)
- [x] Hardware interrupts (PIC) + a periodic timer
- [x] PS/2 keyboard input
- [x] Physical memory: paging, a frame allocator
- [x] A kernel heap + allocator (`alloc` support: `Box`, `Vec`)
- [x] Cooperative multitasking (async/await executor) · [ ] preemptive next
- [x] A tiny in-kernel shell
- [x] A simple ramdisk / read-only filesystem (`ls`, `cat`)
- [ ] Preemptive scheduling; a real disk-backed filesystem; more commands

## How it boots

The kernel targets the bare-metal triple `x86_64-unknown-none` (no host OS, no SSE, red zone disabled), built **static / non-PIE** so the bootloader can load it. Rust's `build-std` compiles `core` for that target, and the [`bootloader`](https://crates.io/crates/bootloader) crate provides a BIOS bootloader that sets up long mode and jumps to `_start`. `bootimage` links the two into a single bootable disk image.

## Build & run

Requires a nightly Rust toolchain (pinned in `rust-toolchain.toml`), `bootimage`, and QEMU.

```bash
# one-time setup
rustup component add rust-src llvm-tools-preview
cargo install bootimage

# build a bootable disk image and launch it in QEMU
cargo run
```

`cargo run` builds `target/x86_64-unknown-none/debug/bootimage-aetheros_kernel.bin` and boots it in QEMU. Kernel output is mirrored to the terminal via `-serial stdio`, so the boot can be verified headlessly.

## Layout

```
src/main.rs        kernel entry point (_start) + panic handler
src/vga_buffer.rs  VGA text-mode driver and print macros
src/serial.rs      UART 16550 serial driver (host-side logging)
src/interrupts.rs  IDT, CPU exceptions, PIC, timer + keyboard handlers
src/memory.rs      paging: page-table access, translation, frame allocator
src/allocator.rs   kernel heap region + hand-written bump allocator
src/task/          async Task type + cooperative round-robin executor
src/shell.rs       interactive command shell (line buffer + commands)
src/fs.rs          read-only in-memory filesystem (ramdisk)
.cargo/config.toml build-std + target + bootimage runner config
docs/              boot screenshots + devlog/ (plain-English notes per milestone)
```

## Credit & approach

The early scaffolding follows the well-trodden path of Philipp Oppermann's excellent [*Writing an OS in Rust*](https://os.phil-opp.com/). Code is heavily commented because the point is to understand it, not just run it.

---

*Built in the open, one commit at a time.*
