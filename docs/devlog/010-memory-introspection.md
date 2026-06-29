# Devlog 010 — Memory introspection (`mem`)

**Milestone:** the OS can now report on its own memory — a `mem` command that shows how much of the heap is in use and how much physical RAM the machine has. It's a tiny `free`/`/proc/meminfo`.

## Why an OS needs to see itself

An operating system manages resources, so it has to be able to *measure* them. Every real OS exposes memory stats (Linux's `free`, `/proc/meminfo`; Windows' Task Manager). This milestone gives AetherOS the same kind of visibility, from two sources we already built:

- **The heap**, from the allocator (Stage 5).
- **Total RAM**, from the boot memory map (Stage 4).

## Heap stats: reading the allocator

The bump allocator already tracks everything we need — it just wasn't exposed. `heap_stats()` locks the allocator and reads three numbers:

- **bytes in use** = how far the bump pointer has advanced from the heap start,
- **total** = the heap size,
- **live allocations** = the count it keeps for deciding when to reset.

There's a subtlety worth understanding, and the demo shows it. Right after boot, `mem` reports **0 bytes used** — because every allocation made during boot (the Stage 5 `Box`/`Vec`/`String` demo) was dropped, the live count hit zero, and the bump allocator reset its pointer to the start. But run `ls` first and `mem` then reports **664 bytes across 7 allocations** — because listing files initializes the filesystem, whose `Vec<File>` and `String`s live in a `static` and are *never* freed. So the number isn't arbitrary: it's exactly the filesystem's data sitting on the heap. The tool reflects the real behavior of the allocator underneath it.

## RAM stats: reading the memory map

The bootloader hands us a **memory map** — a list of physical regions, each tagged (usable, reserved, etc.). To get total RAM, sum the lengths of every `Usable` region. The catch is timing: that map is only available in `kernel_main` at boot, so I tally it there and stash the total in an `AtomicU64` the shell can read back any time. (An atomic, not a lock, because it's written once and read occasionally — no need for anything heavier.) On the default QEMU machine that comes out to ~120 MiB usable.

## Small feature, real lesson

`mem` is a handful of lines, but it ties two subsystems together and surfaces something true about how they work — including the bump allocator's reset behavior, which is exactly the kind of detail that's easy to gloss over until a tool makes it visible. Good instrumentation doesn't just report numbers; it teaches you about the system you built.

## Next

The bigger remaining pieces: a real disk driver (so files survive reboots) and preemptive scheduling (the timer forcibly switching tasks). Both are larger, multi-step efforts — the kind worth taking slowly.
