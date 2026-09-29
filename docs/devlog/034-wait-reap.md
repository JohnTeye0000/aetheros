# Day 34 — wait/reap: a real parent-child relationship

Stage 33's `spawn` returned a pid and nothing else — a spawned process was exactly as independent as one the shell started, no different. Today a spawning process can actually **wait for its child** and learn how it ended.

## The syscall

`wait(pid_filter, status_out_ptr) -> pid, or -1` (rax = 12). `pid_filter` 0 means "any child of mine"; a specific pid waits for just that one. If `status_out_ptr` is non-zero, the exit status is written there as a u32 (`0xFFFFFFFF` = the child faulted rather than calling `exit`). Blocks by sleeping, same as `recv`/`readkey`.

## What makes it correct rather than just "eventually returns"

1. **No infinite block.** Before sleeping, `wait` checks whether a matching child could *ever* show up — alive right now, or already finished and waiting to be collected. If neither, it returns -1 immediately instead of hanging forever on a child that doesn't exist.
2. **A receipt survives the child's teardown.** `spawn_bytes` now records `parent` on the `Proc`. When a process exits, `user_thread` drops a small `ExitRecord {pid, parent, status}` into a capped table *before* the process's address space is freed — so a parent that calls `wait` after the child is long gone (frames released, `Proc` removed) still gets the right answer.
3. **Shell-launched programs don't leak records.** `parent == 0` means "the shell started this, nobody is waiting" — no exit record is kept, so `run`-ing programs from the shell forever doesn't slowly fill the table.

## Demo: `waiter`

```
waiter: wait() with no children correctly returned -1
waiter: spawned count, now blocking in wait() until it exits...
#1
#2
#3
#4
#5
waiter: child reaped, its exit status was: 0
```

It calls `wait` with zero children first (proving the no-hang guarantee), *then* spawns `count` and blocks until it genuinely finishes — the five ticks print before "reaped" because `waiter` really is asleep, not polling.

## Verified

Under `-cpu max`, `waiter`, `launcher`, `producer`/`consumer`, `nx`, `poke`, `hello` all ran together with no regressions; `mem`'s frame count matched a clean run (144, same shape as before) — no leaked records or frames from the new bookkeeping.

## Limits

`ExitRecord`s are capped at 32 and pruned oldest-first if a parent never collects them (best-effort, not a guarantee); no `SIGCHLD`-equivalent notification (a parent must actually call `wait`, not get told); no process groups.
