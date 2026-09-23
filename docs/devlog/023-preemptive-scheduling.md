# Devlog 023 — Preemptive multitasking

**Milestone:** the kernel now runs several threads and switches between them *by force*, on every timer tick, whether or not any of them ever asks. Three "worker" threads spin in tight loops that never yield, and all three make progress.

## Cooperative vs. preemptive, concretely

Stage 6's async executor was cooperative: a task runs until it awaits. That's safe and simple, and it has one fatal weakness — a task that never yields owns the CPU forever. Preemption removes the weakness by removing the choice: the timer interrupt (Stage 2) fires ~18 times a second no matter what the running code is doing, and the kernel uses that interrupt to swap threads.

The proof is the `threads` command:

```
aether> threads                          aether> threads   (a few seconds later)
   1  worker-a  running  10  2165622        1  worker-a  running  37  8388842
   2  worker-b  ready     9  2136238        2  worker-b  ready    36  8517186
   3  worker-c  ready     9  2151514        3  worker-c  ready    36  8558950
```

Each worker's body is literally `loop { counter += 1 }`. There is no yield, no sleep, no call. If all three counters climb, the CPU *must* have been taken from each of them.

## The context switch: fifteen instructions

A thread that isn't running is just a stack plus one saved stack pointer. `switch_context(old, new)` pushes the six callee-saved registers (`rbp rbx r12–r15`) onto the current stack, stores `rsp` into `*old`, loads `rsp` from `new`, pops the six registers off the *new* stack, and executes `ret`. That `ret` returns into wherever the new thread last called `switch_context` from. Each thread experiences `switch_context` as an ordinary function call that takes a very long time to return. (Caller-saved registers don't need saving: the compiler already assumed a function call would clobber them.)

The switch is made *inside the timer interrupt handler*, so a suspended thread is always parked mid-handler with its interrupt frame still on its own stack. When it's resumed it finishes the handler and `iretq`s to the exact instruction the timer interrupted — which never noticed anything happened.

## How a thread that has never run gets started

There are no saved registers to restore for a new thread, so `spawn` **forges a stack that looks like a suspended thread**: six zeroed register slots, then a return address pointing at `thread_start`. `thread_start` enables interrupts (`sti`; the first entry into a thread comes from inside an interrupt handler, where they're off), moves the argument slot into `rdi`, and calls the entry function. Getting the alignment right takes care: the forged frame is laid out so that after `ret` pops the return address, `rsp` sits on a 16-byte boundary, which is what the System V ABI needs at a `call`.

## Two bugs waiting to happen (avoided on purpose)

**EOI before the switch.** The 8259 PIC will not deliver another timer interrupt until it's told the last one was handled. The old handler sent that EOI as its last act. But a *new* thread doesn't come back through the handler — it starts at `thread_start` — so an EOI left until the end would never be sent for that tick and the timer would go silent forever. The handler now sends EOI first, then switches.

**The heap lock and preemption.** The allocator sits behind a spinlock. Suppose thread A is preempted while inside `alloc`, holding that lock, and thread B then allocates. B spins waiting for a lock whose owner is asleep, and with one CPU nobody is left to release it: deadlock. The fix is the same one the print macros got in Stage 2 — hold the lock with interrupts disabled, so the timer can never fire while it's held. Every lock that both threads and interrupt handlers might take needs this rule; the scheduler's own lock only ever lives inside interrupt-disabled code.

## What it costs, visibly

Each thread has its own 32 KiB stack, carved out of the heap (which grew to 512 KiB for this). `mem` afterwards reports 98,832 bytes in use — 3 stacks × 32,768 = 98,304, plus a few hundred bytes of bookkeeping. The stacks show up exactly where they should in the allocator's accounting.

## Limits, honestly

Round-robin with a fixed ~55 ms slice; no priorities, no blocking (a thread can't wait for something without spinning), and finished threads keep their stacks. The next stage adds thread lifecycle management: spawning and killing from the shell, and blocking sleep.
