# Program-hint-guided scheduling in AetherOS: research plan and status

This is the working document for a small systems study built on the AetherOS
kernel. It says what the question is, what has been built to answer it, what has
actually been measured so far, and — just as importantly — what has **not** been
done yet.

## The question

> Can information a program declares about itself (eventually: information a
> compiler derives from its source) help an OS scheduler make better decisions
> than a scheduler that only observes runtime behaviour?

Sub-questions:

1. **Does it help?** Do hint-guided policies improve latency for interactive and
   I/O-bound processes compared with policies that have no hints?
2. **When?** Under which workload mixes does the benefit appear, and when does it
   vanish?
3. **What does it cost?** Throughput of CPU-bound work, fairness, context
   switches, time spent in the scheduler.
4. **How fragile is it?** What happens when hints are missing or wrong?

### Hypotheses (to be attacked, not defended)

- **H1.** With correct hints, a hint-guided scheduler gives interactive processes
  lower tail (p95/p99) wake-up latency than round-robin.
- **H2.** A runtime-inference scheduler (MLFQ) recovers *part* of that benefit
  without hints, so the interesting comparison is hints vs. MLFQ, not hints vs.
  round-robin.
- **H3.** The benefit is concentrated in mixed workloads; with homogeneous
  CPU-bound work, hints change nothing.
- **H4.** Wrong hints make the hint-only policy worse than having no hints, while
  combining hints with runtime feedback (`hmlfq`) limits the damage.

## What has been built (Stage 35 of the kernel)

| piece | where |
|---|---|
| 1 kHz timer (was 18.2 Hz), TSC timestamps calibrated against it | `src/interrupts.rs`, `src/timing.rs` |
| per-thread accounting: response time, CPU time, ready-wait time, every wake-up latency | `src/scheduler.rs` |
| real blocking (wait queues keyed by event) instead of sleep-and-poll | `scheduler::block_current` / `wake_all` |
| idle thread that never takes a time slice | `scheduler::pick` |
| four policies, switchable at runtime | `rr`, `hint`, `mlfq`, `hmlfq` |
| hints carried in the program file (`hint=cpu\|io\|interactive`) | `usermode::parse_axe_full` |
| benchmark workloads `wcpu`, `wio`, `wint` | ring-3 programs in `src/usermode.rs` |
| `bench` command, autorun scripts, CSV output | `src/bench.rs` |
| unattended host harness + analysis + graphs | `experiments/aetherbench.py`, `experiments/analyze.py` |
| SSE register save/restore across context switches (floats in user programs) | `src/scheduler.rs` (devlog 036) |
| PlainLang → x86-64 compiler and a static hint classifier | PlainLang repo: `plainaxe.py`, `plainhint.py` (devlog 037) |
| PlainLang workloads whose hints come from the compiler | `experiments/plainlang/*.plang` |

### The policies

| | priority from | notes |
|---|---|---|
| **rr** (S0, baseline) | nothing | one queue, fixed quantum Q |
| **mlfq** (runtime only) | observed behaviour | 3 levels, quantum Q·2^level, demote after using a level's allotment (survives blocking, so it can't be gamed), all threads boosted to the top every 200 ms |
| **hint** (S1, static only) | the program's declared hint | interactive → level 0, io/none → 1, cpu → 2; a thread waiting > 100 ms is aged to the top (anti-starvation) |
| **hmlfq** (S2, both) | hint as MLFQ's *starting* level | otherwise identical to `mlfq` |

### The workloads

| program | behaviour | true hint |
|---|---|---|
| `wcpu` | ~80 M instructions of pure computation, never blocks | cpu |
| `wio` | 30 rounds: ~4.8 ms compute, then 4 ms blocked ("simulated I/O") | io |
| `wint` | 30 rounds: wait 15 ms for an "event", then ~0.5 ms of work | interactive |

| workload | processes |
|---|---|
| `cpu4` | 4 × wcpu (homogeneous control) |
| `mixed` | 2 × wcpu + 2 × wio |
| `interactive` | 3 × wcpu + 1 × wint |
| `all` | 2 × wcpu + wio + wint |
| `scaleN` | N × wcpu + 1 × wint |

### Metrics

Per process: response time (created → first run), turnaround, CPU time, time
spent ready-but-waiting, context switches, preemptions, blocks, and **wake-up
latency** (woken → actually running) for every wake-up, reported as
p50/p95/p99/max. Per run: makespan, total context switches, cycles spent in the
scheduler's decision code. Across runs: mean ± 95% CI (t-distribution, one value
per run), and Jain's fairness index over the CPU-bound processes.

## How to reproduce

```bash
cargo bootimage
# the PlainLang workloads are compiled with the PlainLang repo's plainaxe.py:
#   python ../plainlang/plainaxe.py experiments/plainlang/p_cpu.plang   (etc.)
python experiments/aetherbench.py --script experiments/plans/main_all.txt        --out experiments/results/main_all --files experiments/plainlang/*.axe
python experiments/analyze.py experiments/results/* --out experiments/report
```

Each plan file is a list of `bench`/`repeat N bench` lines. The harness writes
them onto a data disk as the kernel's `autorun` file, boots QEMU unattended
(the kernel powers itself off when done), and parses the serial log.

## Measurement problems found and fixed before any result was kept

Devlog 035 has the details. The harness itself had to be debugged:

* a starved **benchmark launcher** made processes start late. One early "result"
  said wrong hints *improved* turnaround by 27%. The launcher now runs as a
  top-priority system thread, and every result from before the fix was
  discarded;
* the **first run of each boot** was ~25% slow, because the kernel's frame
  allocator was O(n) per frame (it only showed up before freed frames were
  available for reuse). It is now O(1). `analyze.py` flags any run more than 5%
  from its configuration's median;
* a **parser bug** silently dropped 1 row in 24;
* kernel bugs (heap fragmentation, unreaped threads, an interrupt/allocator
  deadlock) made long batches crash or hang. Each was found from the harness's
  own diagnostics.

## Threats to validity (read before believing any number)

1. **Emulated, not real, hardware.** Experiments run under QEMU's software CPU
   with `-icount shift=3,sleep=off`: guest time advances 8 ns per executed
   instruction, independent of host load. That makes runs reproducible (two
   boots with identical inputs agreed to within 0.1% in our check), but it
   means **every instruction costs the same**. No caches, no pipeline, no
   memory-latency effects. Absolute times are not real-hardware times. Only
   *relative* comparisons between policies under the same conditions carry
   meaning.
2. **Synthetic workloads.** `wio`'s "I/O" is a timed sleep, not a device. `wint`'s
   "events" arrive on a fixed 15 ms schedule rather than from a person. Real
   programs have phases, mixed behaviour and data-dependent timing.
3. **Two kinds of hints.** The built-in workloads (`wcpu`, `wio`, `wint`) have
   hand-assigned hints: that tests "does correct static information help?". The
   PlainLang workloads (`pl_*`) carry hints derived by `plainhint.py` from their
   source: that tests the compiler half. The classifier is a simple heuristic
   with stated thresholds, not a general program analysis. Two PlainLang
   programs (`p_phase`, `p_trick`) were written specifically to fool it.
4. **Policy design choices matter.** Results depend on Q = 10 ms, 3 levels,
   200 ms boost, 100 ms aging. The quantum sweep varies Q; the others are fixed
   for now and should be swept before strong claims are made.
5. **Small scale.** One CPU, at most 15 processes, seconds-long runs.
6. **The researcher wrote the baselines.** A hint scheduler beating a weak MLFQ
   proves little. The MLFQ follows the textbook rules (OSTEP ch. 8), but a
   reviewer would rightly ask for comparison against a production-style
   scheduler (e.g. CFS/EEVDF-like fair queuing). That's a known gap.

## Not done yet (the honest list)

- [x] **PlainLang → AetherOS backend** (`plainaxe.py`): hand-encoded x86-64,
  differential-tested on 232 programs against the interpreter inside the kernel.
- [x] **Compiler-derived hints** (`plainhint.py`), with a written rationale per
  program.
- [x] **SSE state save/restore**, verified with a negative control.
- [ ] **Hint-classifier accuracy on real programs**, not just five workloads I
  wrote. E.g. classify every PlainLang lesson/example and compare with measured
  behaviour.
- [ ] **A fair-queuing baseline** (CFS/EEVDF-style) to compare against.
- [ ] **Sweeps of boost period and aging threshold.**
- [ ] **Literature review**: reading the papers below and writing down, in your
  own words, what each one did and how this study differs. Needs to be done
  before claiming anything is novel.
- [ ] **A faculty reviewer.**

## Reading list (starting point)

I'm confident each of these exists as listed. **Check every citation yourself
before using it**, and read the paper, not a summary.

Scheduling foundations
- Corbató, Merwin-Daggett, Daley. *An Experimental Time-Sharing System.* AFIPS
  Spring Joint Computer Conference, 1962. (CTSS, where multi-level feedback
  scheduling comes from.)
- Arpaci-Dusseau & Arpaci-Dusseau. *Operating Systems: Three Easy Pieces*, chapters
  on scheduling and the multi-level feedback queue. (The MLFQ rules used here.)
- Waldspurger & Weihl. *Lottery Scheduling: Flexible Proportional-Share Resource
  Management.* OSDI 1994.
- Stoica & Abdel-Wahab. *Earliest Eligible Virtual Deadline First.* Old Dominion
  University tech report, 1995. (The basis of Linux's current EEVDF scheduler.)
- Duda & Cheriton. *Borrowed-Virtual-Time (BVT) Scheduling: Supporting
  Latency-Sensitive Threads in a General-Purpose Scheduler.* SOSP 1999. (Very
  close to this study's question: favouring latency-sensitive threads.)

Applications telling the OS things
- Patterson, Gibson, Ginting, Stodolsky, Zelenka. *Informed Prefetching and
  Caching.* SOSP 1995. (Application-disclosed hints, for I/O rather than CPU.)
- Anderson, Bershad, Lazowska, Levy. *Scheduler Activations.* SOSP 1991.
- Engler, Kaashoek, O'Toole. *Exokernel.* SOSP 1995.
- Bershad et al. *Extensibility, Safety and Performance in the SPIN Operating
  System.* SOSP 1995. (Language-based OS extension.)
- Hunt & Larus. *Singularity: Rethinking the Software Stack.* ACM SIGOPS OSR,
  2007. (Language/OS co-design.)

Program behaviour from analysis
- Sherwood, Perelman, Hamerly, Calder. *Automatically Characterizing Large Scale
  Program Behavior.* ASPLOS 2002. (Program phases. Relevant because a single
  static hint can't capture a program whose behaviour changes.)

Modern latency-focused scheduling
- Ousterhout et al. *Shenango.* NSDI 2019.
- Kaffes et al. *Shinjuku: Preemptive Scheduling for µsecond-scale Tail
  Latency.* NSDI 2019.
- Fried et al. *Caladan.* OSDI 2020.
- Humphries et al. *ghOSt: Fast & Flexible User-Space Delegation of Linux
  Scheduling.* SOSP 2021.
- Dean & Barroso. *The Tail at Scale.* CACM 2013. (Why p99 matters more than the
  mean.)

Measurement
- Jain, Chiu, Hawe. *A Quantitative Measure of Fairness and Discrimination for
  Resource Allocation in Shared Computer Systems.* DEC TR-301, 1984. (Jain's
  fairness index, used in `analyze.py`.)

Search terms for finding the closest prior work on the compiler side:
"compiler-directed scheduling", "application hints CPU scheduling",
"static program classification scheduling", "language runtime OS co-design".
