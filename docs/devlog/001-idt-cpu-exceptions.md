# Devlog 001 — CPU exceptions via an Interrupt Descriptor Table (IDT)

**Milestone:** the kernel can now survive a CPU exception instead of rebooting.

## What problem this solves

On day 1, an unhandled fault rebooted the whole machine — that was the "triple fault" reset loop we debugged. A triple fault happens in three steps:

1. The CPU hits an **exception** (something went wrong — a breakpoint, a divide-by-zero, an invalid instruction, a bad memory access).
2. It looks for a handler. If there's none, that's a **double fault** (a fault while handling a fault).
3. If the double fault also has no handler, that's a **triple fault** → the CPU gives up and resets.

Today we stop the bleeding at step 1 by giving the CPU somewhere to go when an exception happens.

## The key idea: the IDT

The **Interrupt Descriptor Table** is a table the CPU hardware reads. It has one slot per exception "vector" (vector 3 = breakpoint, vector 8 = double fault, vector 14 = page fault, and so on). Each slot holds the address of a function to call when that exception fires.

We:
1. Build an IDT in `src/interrupts.rs`.
2. Put our `breakpoint_handler` in slot 3 and our `double_fault_handler` in slot 8.
3. Run the `lidt` instruction (via `IDT.load()`) to tell the CPU "here's the table."

After that, the CPU knows what to do when an exception occurs.

## Proving it works

In `_start` we now *deliberately* run `int3` — the one-byte instruction that raises a breakpoint exception (it's how debuggers pause a program). If our IDT is wired up correctly:

- the CPU jumps into `breakpoint_handler`, which prints a line, and
- when the handler returns, **execution continues at the instruction right after `int3`.**

The serial log shows exactly that — "breakpoint (#BP) handled" followed by "...and we are back". Surviving and continuing past an exception is the whole point.

## Two details worth understanding

- **`extern "x86-interrupt"`** — exception handlers use a special calling convention. The CPU pushes extra state (an `InterruptStackFrame`) before calling the handler, and the handler must end with `iretq`, not a normal `ret`. That ABI is unstable in Rust, so `main.rs` has `#![feature(abi_x86_interrupt)]`.
- **The double-fault handler returns `!` (never returns).** You can resume from a breakpoint, but a double fault means the CPU was already in trouble — there's nothing safe to return to, so we only report and stop. (A hardened version runs it on its own dedicated stack via the GDT/TSS so even a stack overflow can't escalate it — that's a later milestone.)

## Next

Hardware interrupts: program the 8259 PIC and wire up a periodic timer interrupt — the first time the kernel reacts to the *outside world* on its own, which is the groundwork for keyboard input.
