# Devlog 012 — Surviving a stack overflow (GDT + TSS + IST)

**Milestone:** the kernel now catches a **stack overflow** instead of rebooting. This closes a loose end left open all the way back in Stage 1's devlog, and it's the most "real OS hardening" milestone so far.

## The problem: a fault while handling a fault while handling a fault

Back in Stage 1 we added a double-fault handler so unhandled exceptions couldn't triple-fault the machine. But there was a hole: **the handler runs on the current stack.** So consider a *stack overflow*:

1. The kernel recurses too deep and the stack pointer runs past the bottom of the stack, into the **guard page** (an unmapped page the bootloader places there).
2. Touching it → page fault. To deliver the page fault, the CPU pushes an exception frame... onto the stack... which is already over the edge → another fault → **double fault**.
3. The CPU tries to run our double-fault handler — by pushing *its* frame onto the same broken stack → fault again → **triple fault** → the machine resets.

So the one handler meant to save us gets taken down by the very condition it's supposed to report. The demo without this fix is a silent reboot.

## The fix: give the handler its own stack

x86-64 has a feature exactly for this: the **Interrupt Stack Table (IST)**, part of the **Task State Segment (TSS)**. The IST holds up to 7 known-good stack pointers. You can tell an IDT entry: "when this exception fires, *unconditionally switch to IST stack N* before doing anything else." Now the double-fault handler always starts on a fresh, valid stack, no matter how wrecked the current one is.

Wiring it up takes three pieces:

1. **A TSS** with one IST entry pointing at a dedicated 20 KiB stack (a `static` byte array). Stacks grow down, so the CPU gets the *top* address.
2. **A GDT** to hold the TSS. x86-64 mostly ignores segmentation, but a TSS can only be referenced through a GDT segment descriptor — so we build a minimal GDT (a kernel code segment + the TSS segment), load it, reload `CS`, and `ltr` the TSS.
3. **Point the IDT entry at the IST slot**: `idt.double_fault.set_handler_fn(...).set_stack_index(0)`.

Order matters: the GDT/TSS must be loaded *before* the IDT, since the IDT entry references an IST index that has to already exist. So `gdt::init()` runs first in boot.

## Proving it

The `crash` command deliberately recurses forever (with a volatile read after each call so the compiler can't turn it into a harmless loop). The stack overflows, the double fault fires — and the handler *runs*, printing the panic and the faulting frame, then halts. **The handler executing at all is the proof:** if the IST switch hadn't worked, there'd be no output, just a reboot. Seeing the red panic screen means the mechanism did its job.

## Why this one is worth understanding

This is the milestone that most looks like what a real OS has to worry about — not a feature users see, but *robustness*: making sure the kernel's own safety nets can't be knocked out by the failures they're meant to catch. The GDT, TSS, and IST are core x86-64 structures, and "how do you handle a stack overflow in kernel space" is a genuinely good systems question. Now there's a real answer here, and a command that demonstrates it live.

## Next

The remaining heavyweight milestones — preemptive scheduling and a real disk driver — stay on the list for slower, multi-session work.
