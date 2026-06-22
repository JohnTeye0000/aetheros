# Devlog 006 — Cooperative multitasking (async/await)

**Milestone:** the kernel can now run more than one task "at the same time" on a single CPU. Two async tasks interleave, each making progress in turn.

## Two kinds of multitasking

- **Preemptive:** a timer interrupt forcibly pauses a task mid-execution and switches to another. The tasks don't cooperate — the kernel takes control away. Powerful, but it requires saving/restoring full CPU state and careful locking.
- **Cooperative:** each task runs until it *voluntarily* yields control, then the next one runs. Simpler and safe (a task is never interrupted at a random instruction), at the cost of trusting tasks to yield. This milestone does the cooperative kind; preemptive is next.

## How async/await gives us this almost for free

A Rust `Future` is a computation you *poll*: each poll does a little work and returns `Ready(value)` or `Pending` ("not done, poll me again"). An `async fn` compiles into exactly such a future, and every `.await` inside it is a point where it can return `Pending` — i.e., a place it can yield.

So cooperative multitasking is just:

1. **`Task`** — a future wrapped in `Pin<Box<...>>`. (Pinned because a future can hold references into itself, so once polled it must never move.)
2. **An executor** — a queue of tasks. Pop one, poll it once. If `Ready`, it's done — drop it. If `Pending`, push it to the back. Repeat until the queue is empty.
3. **A yield point** — `yield_now().await`. My `YieldNow` future returns `Pending` the first time it's polled and `Ready` the second. So a task that calls it hands control back to the executor, which runs the next task before coming back.

The demo spawns two tasks that each print three steps with a `yield_now().await` between them. The output is `A1, B1, A2, B2, A3, B3` — they interleave. That interleaving *is* the multitasking.

## The weird bit: the Waker

`Future::poll` takes a `Context`, which carries a `Waker`. A waker is how a *real* executor avoids busy-looping: a blocked task (say, waiting on the keyboard) calls `waker.wake()` when its data arrives, telling the executor "re-schedule me." Building one means filling a `RawWakerVTable` — four raw function pointers (clone, wake, wake_by_ref, drop), because this is the lowest level of the async machinery, below any allocation.

My executor re-polls every `Pending` task each round anyway (round-robin), so it doesn't actually *need* a smart waker — I supply a **no-op waker** whose vtable functions do nothing. That keeps this milestone focused. A real executor would use the waker to sleep until there's work (e.g. `hlt` until the keyboard interrupt wakes a task), which is a natural future upgrade.

## Why this matters

This is a scheduler in miniature — the core idea behind running many programs on one machine. The next step (preemptive multitasking) makes the switch involuntary using the timer interrupt we built in Stage 2, so a misbehaving task can't hog the CPU. After that: a shell, where you type commands and the kernel runs them.
