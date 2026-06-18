# Devlog 003 — PS/2 keyboard input

**Milestone:** the kernel now responds to you typing. Press a key and it appears on screen.

## How a key becomes a character

It's a four-step pipeline, and each step is a different piece of hardware or code:

1. **You press a key.** The keyboard sends a *scancode* — a small number identifying the key — to the PS/2 controller chip. It sends one code when the key goes down and another when it comes up.
2. **The PS/2 controller raises IRQ1.** It parks the scancode in I/O port `0x60` and signals the interrupt controller (PIC) that input is waiting.
3. **The CPU jumps to our handler** (vector 33 in the IDT). We *must* read port `0x60` — the controller will not send another keyboard interrupt until that byte is read.
4. **We decode the scancode** with the `pc-keyboard` crate, which is a little state machine: it tracks shift/caps, handles multi-byte sequences, and turns `0x1E` into `'a'`. We echo the result to screen and serial.

The key idea: the keyboard doesn't hand us letters. It hands us *numbers that identify physical keys*, and turning those into text is software's job. That's why keyboard layouts (US, UK, Dvorak) are a software setting — same scancodes, different letters.

## The bug I hit (worth understanding)

The first build compiled and booted fine, the timer ticked... but typing did nothing. No crash, just silence.

The clue: the **timer** (IRQ0) worked but the **keyboard** (IRQ1) didn't. If the IDT or interrupts were broken, the timer wouldn't work either. So the problem had to be specific to IRQ1.

The cause was the **PIC interrupt mask**. The PIC has a mask register where each bit disables one IRQ line (a 1 = "ignore this line"). When I called the PIC's `initialize()`, it restored whatever mask the BIOS had left behind — and SeaBIOS leaves the keyboard line *masked off*. So the keyboard was raising IRQ1, but the PIC was silently dropping it before the CPU ever saw it.

The fix: after initializing the PIC, explicitly write the mask myself — enable IRQ0 (timer), IRQ1 (keyboard), and IRQ2 (the cascade to the second PIC), disable the rest:

```rust
pic1_mask.write(0b1111_1000); // 0 = enabled; bits 0,1,2 cleared
```

Lesson: "it compiled and booted" is not "it works." A masked interrupt fails *silently* — no error, no crash, just nothing. Being explicit about which interrupts you accept is both the fix and good practice.

## How I verified it headlessly

No physical keyboard in an automated test, so I drove QEMU's monitor with `sendkey a`, `sendkey e`, … to inject keystrokes, and checked that the serial log echoed `a e t h e r o s`. The screenshot shows the same word typed on the emulated screen.

## Next

Physical memory — paging and a frame allocator. This is the big one: teaching the kernel to manage RAM itself, the foundation for a heap (so we can use `Box`/`Vec`) and eventually separate processes.
