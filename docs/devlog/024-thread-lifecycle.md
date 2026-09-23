# Devlog 024 — Thread lifecycle: spawn, sleep, kill

**Milestone:** threads can now be created, put to sleep, and destroyed on demand from the shell, and a dead thread's memory actually comes back.

```
aether> spawn ticker      aether> ps
aether> spawn spin          id  name         state         switches  progress
                             0  kernel_main  ready               57  -
                             1  ticker       sleeping(12)        13  12
                             2  spin         running             51  11385556
```

## Sleeping is not spinning

Stage 23's workers burned CPU in loops. A real thread usually wants to *wait*. `sleep_ticks(n)` marks the calling thread `Sleeping` with a wake-up tick and immediately hands the CPU to someone else — by calling `switch_context` directly, as an ordinary function call, rather than waiting for the timer. That's the second way a thread can be suspended (the first is being preempted mid-interrupt), and `switch_context` doesn't care which: it only swaps stacks. The sleeper resumes by simply *returning* from `sleep_ticks`.

Every scheduling decision first wakes any sleeper whose tick has arrived, then picks the next `Ready` thread round-robin. While a thread sleeps it costs nothing.

The `ticker` demo thread is `sleep(18 ticks); log; repeat`. The serial log shows it waking at ticks 434, 452, 470, 488… — **exactly 18 apart**, every time, while a `spin` thread burns the rest of the CPU beside it. `ps` shows the remaining sleep as a countdown: `sleeping(12)`.

## Killing, and getting the memory back

`kill <id>` marks a thread `Dead`; the scheduler never picks it again. `reap` then frees its 32 KiB stack by dropping the thread's `Box`. Two details matter:

- **Never free the stack you're standing on.** The shell runs inside an interrupt handler *on whichever thread's stack was interrupted*. If you `kill` that very thread, its stack is still in use until the handler returns, so `reap` skips the running thread and it is collected at the next reap.
- **Indices shift.** The scheduler tracks its current thread by index into a `Vec`; removing dead threads moves everything, so `reap` re-finds the current thread by id afterwards.

The heap accounting confirms the stacks really return:

| after | `mem` bytes in use |
|---|---|
| spawn ticker + spin | 66,144 |
| `kill 2` (spin) | 33,520 |
| `kill 1` (ticker) | 704 |

Each kill releases ~32 KiB. `kill 0` is refused (thread 0 is `kernel_main`), and an unknown id is reported rather than crashing.

## The caveat, stated plainly

Killing a thread that holds a lock leaves the lock held forever. The demo threads never hold a lock across a preemption point, so they're safe to kill, but this is not a general guarantee. A real kernel needs cleanup hooks or cooperative cancellation. Same for stack overflow: thread stacks live in the heap with no guard page, so a runaway thread would corrupt its neighbour instead of faulting cleanly.

## A correction about how earlier stages were tested

While testing this stage my scripted session mysteriously froze after ~40 keystrokes, every time. It looked like a kernel bug, and it took a while to see it wasn't. The test harness piped QEMU's monitor output but never read it; every simulated key makes the monitor echo text, and a Windows pipe buffer is ~4 KB, so around the 40th key QEMU blocked writing to a full pipe and the whole virtual machine stalled. Discarding that output fixed it instantly (a 46-character line now types fine).

That means an explanation I gave in earlier devlogs — that truncated test sessions were "QEMU running slowly on the host" — was wrong: those were this same deadlock. The kernel results in those stages were unaffected (each was verified from a session that completed), but the reasoning I gave for the flakiness was mistaken. Lesson worth keeping: when a test hangs *deterministically after a fixed amount of input*, suspect a buffer somewhere, not slowness.
