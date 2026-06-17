# Devlog 002 — Hardware interrupts: the PIC and the timer

**Milestone:** the kernel now reacts to the hardware on its own. Until today every line that ran was a line *we* called. Now the timer chip interrupts the CPU ~18 times a second and our handler runs — with nothing in our code asking for it.

## Exceptions vs. interrupts

Stage 1 handled **exceptions** — the CPU complaining about something *we* did (we ran `int3`). Today is about **hardware interrupts** — external devices (timer, keyboard, disk) saying "hey, I need attention." Both end up going through the IDT and a handler; the difference is who starts it. Interrupts are how a CPU does more than one thing: instead of constantly *checking* "did a key get pressed? did the timer tick?" (polling, which wastes the CPU), the device *tells* the CPU when something happens.

## The PIC, and why we remap it

Devices don't wire straight into the CPU. They go through the **8259 PIC** (Programmable Interrupt Controller) — actually two chips chained together, handling 15 interrupt lines (IRQs). IRQ0 is the timer, IRQ1 is the keyboard, and so on.

Here's the catch that bit early OS authors: by default the PIC delivers its IRQs on CPU vectors 0–15. But the CPU **already reserves vectors 0–31 for exceptions** — vector 8 is the double fault! So a timer tick would look like a double fault. The fix is to **remap** the PIC to deliver IRQs starting at vector 32 (just past the exceptions). That's the `ChainedPics::new(32, 40)` + `initialize()` in `interrupts.rs`. Timer → vector 32, keyboard → 33.

## Three things the handler must get right

1. **Be registered in the IDT** at vector 32 (`idt[32].set_handler_fn(timer_interrupt_handler)`).
2. **Enable interrupts** with the `sti` instruction (`interrupts::enable()`). Until you do this the CPU ignores all hardware interrupts — they stay "masked."
3. **Send End-Of-Interrupt (EOI)** at the end (`notify_end_of_interrupt`). The PIC waits for this acknowledgement before delivering the *next* interrupt on that line. Forget it and the timer fires exactly once, then goes silent forever. (Classic first-OS bug.)

## The deadlock I had to prevent

Enabling interrupts created a sharp new hazard. Our `print!` grabs a lock on the screen. If the timer fires *while the main code is mid-print and holding that lock*, the timer handler — which also prints — tries to grab the same lock and spins forever. The whole kernel freezes. The fix: every print now holds its lock **with interrupts disabled** (`without_interrupts`), so a print can't be interrupted by something that needs the same lock. Small change, but it's the difference between a kernel that runs and one that randomly hangs.

## `hlt`: sleeping instead of spinning

The idle loop changed from `loop {}` (spin at 100% CPU forever) to `loop { hlt() }`. `hlt` halts the core until the *next* interrupt wakes it. So the kernel now sleeps between timer ticks instead of burning the CPU — which is exactly how a real OS idles.

## Next

PS/2 keyboard input (IRQ1, vector 33): catch the keyboard interrupt, read the scancode from the controller, translate it to a character, and echo it. The first time the kernel responds to *you*.
