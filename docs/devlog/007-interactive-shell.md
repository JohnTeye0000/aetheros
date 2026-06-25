# Devlog 007 — An interactive shell

**Milestone:** the OS is now *usable*. Type a command, press Enter, and the kernel runs it. This is the first stage that turns a pile of subsystems into something a person can drive.

## What it pulls together

Nothing here is a brand-new low-level subsystem — that's the point. The shell is where the earlier milestones combine:

- **Keyboard (Stage 3)** delivers each keypress.
- **Interrupts (Stage 2)** are how those keypresses arrive.
- **The screen driver (Stage 0)** echoes what you type and prints results.
- **The timer (Stage 2)** backs the `ticks` command's uptime.

That's a good sign in OS design: once the foundations are solid, features start falling out of combining them.

## How input flows

The kernel spends its life asleep in a `hlt` loop. When you press a key:

1. the keyboard fires IRQ1 → the CPU wakes and runs the keyboard interrupt handler;
2. the handler decodes the scancode to a character and calls `shell::on_char`;
3. the shell appends printable characters to a line buffer (echoing each), handles Backspace, and — on Enter — parses the line and runs the command;
4. the CPU goes back to sleep until the next key.

So the whole shell is **event-driven**: there's no busy "read input" loop burning CPU. It reacts to interrupts and idles the rest of the time, exactly like a real OS waiting for work.

## A deliberate design choice: fixed buffer, no heap

The line buffer is a fixed `[u8; 128]`, not a heap `String` — even though we *have* a heap now (Stage 5). Why? The buffer is written from inside the **interrupt handler**, and doing heap allocation at interrupt time is a known footgun (an interrupt landing while the allocator's lock is held could deadlock). A fixed array sidesteps the whole problem: appending a character can never allocate. Small, boring, and safe beats clever here.

## The commands

Five built-ins, dispatched by matching the first word of the line:

- `help` — list commands
- `echo <text>` — print the rest of the line
- `about` — what this is
- `ticks` — uptime, read from the timer-interrupt counter (proof the subsystems talk to each other)
- `clear` — wipe the screen

Adding a command is now a one-line `match` arm — the framework is the valuable part.

## Why this is the milestone that matters

Up to now, "proof it works" meant reading a serial log. Now it means *using it*: someone types `help` and the OS answers. That's the difference between "a kernel that boots" and "an operating system you can interact with" — and it's the thing you can actually demo.

## Next

A read-only ramdisk filesystem (so commands like `ls`/`cat` have something to read), then preemptive scheduling (the timer forcibly switching tasks, instead of tasks yielding voluntarily).
