# Day 35 — Turning the kernel into an experiment

Goal: make AetherOS a platform for a small scheduling study (see
`experiments/RESEARCH.md`). The question: can a scheduler use information a
*program declares about itself* — eventually, information a compiler derives —
to do better than one that only watches runtime behaviour?

That needed four things the kernel didn't have: precise measurement, real
blocking, swappable policies, and a way to run hundreds of experiments with
nobody at the keyboard.

## What was built

**A clock worth measuring with.** The PIT was ticking at its power-on default
of 18.2 Hz (one tick every 55 ms), far too coarse to schedule or measure
anything latency-sensitive. It's now programmed to 1 kHz. Measurement uses the
CPU's timestamp counter (`rdtsc`), calibrated at boot against 50 PIT ticks.

**Accounting per thread.** Every thread records, in TSC cycles: creation, first
run, CPU time, time spent ready-but-waiting, preemptions, blocks, and a sample of
*wake-up latency* (time from being woken to actually running) for every wake-up.

**Real blocking.** `recv`, `wait`, `readkey` and `send` used to "block" by
sleeping one tick and checking again. Now a thread blocks on a 64-bit wait key
(`block_current`), and whoever changes the thing it waits for calls `wake_all`.
The lost-wake-up problem is avoided the standard single-CPU way: check the
condition and block inside one interrupts-off section.

**An idle thread.** Thread 0 (`kernel_main`, sitting in `hlt`) used to take its
turn in the round robin, so a CPU-bound process lost a whole quantum to a thread
doing nothing. It now runs only when nothing else can.

**Four policies, switchable at runtime** (`sched rr|hint|mlfq|hmlfq [quantum]`):
plain round robin; `hint` (priority from the program's declared hint, with
aging); `mlfq` (textbook multi-level feedback queue: demote after using a level's
allotment, boost everyone every 200 ms); `hmlfq` (MLFQ, but the hint picks the
starting level). Programs declare hints in their file header: `hint=cpu`,
`hint=io` or `hint=interactive`.

**A benchmark harness.** `bench <workload> [policy] [q=N] [hints=on|none|wrong]
[seed=N]` launches a fixed mix of processes, waits for all of them, and prints
machine-readable `RESULT`/`SUMMARY`/`LAT`/`HEAP` lines on serial. `hints=wrong`
deliberately lies to the scheduler, to measure what bad information costs. If a
file called `autorun` is on the data disk, the kernel runs it at boot and powers
itself off. The host script `experiments/aetherbench.py` writes that disk image
(in the kernel's AFS1 format), boots QEMU with `-icount` (guest time advances per
instruction, so results don't depend on host load), and parses the log.
`experiments/analyze.py` turns it into tables with confidence intervals, plus graphs.

## The bugs the harness found

Running hundreds of benchmarks back-to-back stress-tests the kernel harder than
anything before. It found real bugs, each of which would have quietly corrupted
or killed long-running workloads.

1. **Heap exhaustion, part 1: threads were never reaped.** Dead threads were only
   freed when someone typed `ps` or `kill`. Each finished process left its 32 KiB
   kernel stack on the heap. Every batch died around run 45 with
   `memory allocation of 32768 bytes failed`. Fix: `spawn` reaps first.
2. **Heap exhaustion, part 2: fragmentation.** The free-list allocator pushed
   every freed block onto the front of the list and never merged neighbours. After
   thousands of alloc/free cycles the heap was confetti: lots of free bytes, no
   32 KiB piece. Fix: keep the list sorted by address and coalesce adjacent
   blocks on free. Each run now logs heap use, the largest free block and live
   allocations. Across 80 consecutive runs all three stay flat.
3. **A deadlock that took ~70 runs to appear.** A batch would freeze with QEMU at
   100% CPU and no output. I added a stall detector to the harness: when the
   serial log stops growing, it asks QEMU's monitor for the registers and stack,
   and resolves addresses to kernel function names by parsing the kernel ELF's
   symbol table. The dump pointed straight at it. The CPU was spinning in the heap
   allocator's spin lock, with interrupts off (`RFL=00000002`). The allocator
   took its lock *without* disabling interrupts, and the new scheduler appends
   latency samples (a growing `Vec`, so an allocation) inside the timer interrupt.
   If the timer fired while any thread was mid-allocation, the handler spun forever
   on a lock its own CPU held. Fix: the allocator holds its lock with interrupts
   off. (My earlier notes claimed this was already the case. It wasn't.)
4. **A result that was too good: a measurement artifact.** One early result said
   that on four identical CPU-bound processes, *lying* about their hints improved
   mean turnaround by 27%. That's impossible: with all four mislabelled equally
   they all have the same priority, which is just round robin. Investigation: the
   makespan was unchanged, but each process's turnaround ended well before the
   run did, so processes were being *created late*. The benchmark launcher is
   itself a thread. With every workload process mislabelled "interactive" (top
   priority), the launcher starved between launches until the 100 ms aging rule
   rescued it. Fix: the harness's own threads are *system* threads that run first
   under every policy. The artifact disappeared (all three conditions now within
   ~2% of each other), and every result produced before the fix was discarded.
5. **First-run penalty: an O(n) frame allocator.** The first benchmark of every
   boot ran ~25% longer (2190 ms vs 1533 ms, consistently). I first blamed
   one-time CPU setup, and moved it out of the measured region, but that wasn't
   the main cause. Per-process data showed each process in run 1 being *created*
   ~170 ms after the previous one. The physical frame allocator (the design from
   the blog_os tutorial this kernel started from) found the n-th free frame by
   walking the memory map from the beginning on every allocation. That's O(n)
   per frame, and with 2,000+ frames already handed out for the heap, a process
   needing 28 frames cost ~60,000 iterator steps. Later runs reused freed frames
   from a free list (O(1)), so only run 1 was slow. Fix: the allocator now
   remembers its position in the memory map. Run 1 now takes 2694 ms against
   2692 ms for later runs. `analyze.py` now flags any run more than 5% off its
   configuration's median, so a regression like this can't hide in an average.
6. **A silently dropped data row.** The first `RESULT` line of a boot was glued
   to the shell prompt (`aether> RESULT,...`), so the parser skipped it, losing
   1 row in 24 with no error. The parser now finds record tags anywhere in a line.

7. **Stalls that weren't the kernel's fault.** Several batches "hung" over the
   afternoon, always in different places, sometimes after the guest had
   already printed `POWEROFF`. The stall dumps showed healthy guests
   mid-computation. The Windows event log explained it: every stall matched a
   **Modern Standby** episode to the second. For example, one batch's last
   output was 13:27:47 against standby entry at 13:27:28, and the stall was
   detected at 13:57:55, the exact second standby ended. The laptop had frozen
   every process. Because guest time under `-icount` counts instructions, not
   wall-clock time, completed runs are unaffected; interrupted batches were
   rerun. The harness now recognises a host pause (a >30 s gap between its
   2-second polls) and doesn't count it as a guest hang. A theory I tested
   along the way, that PlainLang's SSE-heavy programs were losing register
   state, came back negative: 2,160 SSE-checking processes under heavy host
   load showed zero corruptions.

None of these showed up in interactive use. They needed volume, repetition and
an automated "is this number plausible?" step to surface. That's the strongest
argument I have for building the measurement harness *before* chasing results.

## Methodology notes

* Two boots with identical inputs agree to within 0.1% under `-icount`, so
  runs are near-deterministic. Seeds add 0–3 ms random gaps between launches,
  so repeated runs see different arrival patterns.
* Absolute times are emulated-time, not real-hardware time. Only comparisons
  between policies under identical conditions mean anything. Every instruction
  costs the same 8 ns: no caches, no pipeline.
