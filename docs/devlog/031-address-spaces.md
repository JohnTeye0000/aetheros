# Day 31 — Per-process address spaces

Until now every user program shared one set of page tables and ran one at a time. Now each process has **its own top-level page table (PML4)**, several can run at once, and they cannot see each other — even though they all use the *same virtual addresses*.

## How it works

- **A PML4 per process.** `build_process` allocates a fresh PML4, copies the kernel's entries into it (they point at the same lower-level tables, so the kernel is shared and sits at the same addresses in every address space), then maps that process's private code (read-only) and stack (writable, no-execute) into a slot that is empty in the kernel's table. Every process sees its code at the same `USER_BASE` — but those addresses translate to different physical frames.
- **CR3 switches with the thread.** `scheduler::schedule` now calls `usermode::on_switch_to(next)`: load that thread's PML4 into CR3 (kernel threads get the boot tables back), and point the TSS `rsp0` and the SYSCALL stack mirror at *that* process's kernel stack. Kernel code keeps working across the switch because the kernel half is identical everywhere.
- **Per-process state**: kernel resume stack, arguments, fault info live in a `Proc` record (previously globals), found by thread id.
- **Teardown and reuse.** A tracking frame allocator records every frame a process gets (PML4, page tables, code, stack). When it exits, the thread switches back to kernel tables and returns them all to a new free list in the frame allocator. Processes whose thread was `kill`ed are collected on the next launch — never while their tables are the active ones.

## The test

`run iso <tag>` stores its tag on its stack, sleeps 20 ticks while other processes run, then reads the same address back. Three launched back-to-back:

```
memory at my stack address still holds: alpha    (pid 1)
memory at my stack address still holds: beta     (pid 2)
memory at my stack address still holds: gamma    (pid 3)
```

With the old shared mapping, pid 2's tag would have overwritten pid 1's at that address. Also verified with `-cpu max` (NX/SMEP/SMAP on): every earlier program (`hello`, `cat`, `fast`, `keys`, `poke`, `nx`, `selfmod`) still behaves; `kill` on a running process works and its frames are reclaimed. `mem` now shows physical frames in use: 134 before, 134 after a mix of runs, faults and a kill — no leak.

## Bug found on the way

The TSS is a `repr(packed)` struct, so its `rsp0` field is misaligned; a plain `*ptr = value` write panicked ("misaligned pointer dereference") the first time the scheduler updated it. Fixed with `write_unaligned`.

## Limits

Up to 4 processes; the kernel's PML4 entries are copied at process creation, so a *new* top-level kernel mapping made later wouldn't appear in existing processes (all kernel mappings we rely on exist before the first process); keyboard input goes to whichever process is waiting; no shared memory or IPC yet; TLB is flushed wholesale on every CR3 write (no PCID).
