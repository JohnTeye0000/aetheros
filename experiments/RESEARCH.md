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
| **fair** (added 2026-10-18) | nothing: runtime behaviour only | a CFS-style baseline. Each thread accumulates virtual runtime (CPU time used); the least-served runnable thread runs; a slice is Q / (runnable threads), at least 1 ms; a waking thread is credited for sleeping (up to Q/2) and preempts the running one if it is more than 1 ms behind. No weights or nice levels, one CPU, no deadlines (so not EEVDF) |

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

## Results so far (built-in workloads)

Full tables with 95% CIs: `report/report.md`. Graphs: `report/*.png`. 540 runs,
72 configurations, 10 seeds each for the main matrix (5 for the sweeps). The
automated plausibility checks found 0 problems (no run more than 5% off its
configuration's median; every process exited cleanly). Base quantum 10 ms unless
stated. "Latency" = p95 wake-up latency of the process class named.

**F1 — Correct hints cut interactive latency by two to three orders of
magnitude *relative to round robin and MLFQ* (H1 supported against those two
baselines; see F8 for what happens against a CFS-style one).** Interactive p95 latency is 0.03 ms with `hint`, vs
10–15 ms under `rr` and 6–15 ms under `mlfq` (workloads `all`, `interactive`).
It stays under 0.3 ms with 10 CPU-bound competitors (`rr` 85 ms, `mlfq` 45 ms),
and it doesn't depend on the quantum (2–50 ms), where `rr`/`mlfq` latency grows
with it. The mechanism isn't subtle: a woken interactive process has strictly
higher priority, so it preempts within one timer tick.

**F2 — Runtime inference (MLFQ) recovers little of that, and hurts I/O-bound
work (H2 mostly *not* supported).** MLFQ helps a little in `all` (6 ms vs
10 ms) and a lot only at a very small quantum (0.23 ms at q = 2 ms). It doesn't
help in `interactive` (15 vs 15 ms). It makes the I/O-bound process *worse*
than round robin (p95 36 ms vs 16–21 ms). Each `wio` burst is ~4.8 ms of compute
at 50% duty cycle, so MLFQ's allotment accounting (designed so a process can't
stay high by yielding just before its quantum ends) sinks it to the bottom
level within a few bursts. MLFQ's 200 ms boost does cap the damage at scale
(45 ms vs `rr`'s linear growth to 85 ms).

**F3 — With nothing to distinguish, nothing changes (H3 supported).** On
`cpu4` (four identical CPU-bound processes), turnaround is within 1% across all
policies and hint modes. Context switches differ 3× (89 for `hint` vs 277 for
`rr`), because `hint` gives CPU-labelled processes a 4× longer quantum.

**F4 — Wrong hints are worse than no hints, and runtime feedback absorbs them
(H4 supported, with a twist).** With deliberately wrong hints, `hint` gives the
interactive process 105 ms p95 latency, 7–10× worse than round robin. That's
bounded only by the 100 ms aging rule. `hmlfq` with wrong hints behaves almost
exactly like `hmlfq` with correct hints, and like plain `mlfq`. The twist:
runtime feedback erases the *benefit* of correct hints just as thoroughly as
the harm of wrong ones. The 200 ms boost resets everyone to the top level and
demotion quickly overrides the starting level. In this design the hint barely
survives contact with MLFQ.

**Costs.** Scheduler decision time is 0.3–0.6% of runtime for every policy at
the default quantum (up to 1.5% for round robin at q = 2 ms). With
correct hints, CPU-bound *mean* turnaround is slightly lower than `rr` (1382 vs
1515 ms in `all`), but that's because the longer CPU quantum lets one CPU-bound
process finish early. Fairness among CPU-bound processes over the run drops
(Jain index 0.94 in `interactive`, 0.93 in `scale4`, vs 1.00 for `rr`). "No
hints" isn't the same as round robin: every process lands on the middle level,
whose quantum is 2×, so latency doubles.

## Revision after adding a CFS-style baseline (2026-10-18)

Finding F2 said runtime inference doesn't recover what hints give. That was
true of *MLFQ*. It was never tested against the scheduler real systems use.
After adding `fair` (a CFS-style virtual-runtime scheduler, which uses **no
hints**) and running it over the same matrix (795 runs in total now, zero
plausibility warnings), the headline changes. Findings F1-F7 stand as stated
*against rr and mlfq*, but H1's practical claim does not survive F8.

**F8 — A hint-free CFS-style scheduler matches the hint scheduler on
interactive latency.** Interactive p95 wake-up latency: `fair` 0.05 ms, `hint`
0.03 ms, `rr` 10 ms, `mlfq` 6 ms (workload `all`). It holds with ten CPU-bound
competitors (`fair` 0.09 ms, `hint` 0.25 ms, `rr` 85 ms) and at every quantum
from 5 to 50 ms (0.05-0.06 ms). At q = 2 ms it is 1.05 ms, because the slice
floor (1 ms) then limits how fast it can react. The mechanism is the same insight as the hint
scheduler's, but *measured* instead of declared: a process that has been
sleeping has used little CPU, so its virtual runtime is small, so it preempts
the running process. No program had to say what it was.

**F9 — `fair` handles the programs that broke the hint scheduler.** The
phase-changing PlainLang program (`pl_phase`, F6) saw 70.7 ms p95 latency under
`hint` and sees 0.05 ms under `fair`. The misclassified program (`pl_trick`, F7)
doesn't harm its neighbour under `fair`: the genuinely interactive process is
at 0.05 ms, against 2.88 ms under `hint`. Runtime measurement can't be fooled
by a wrong label, because it never reads one.

**F10 — Hints still win on cost, and on I/O-bound latency at large
quanta.** `fair` pays for its fine-grained fairness:
* *Context switches*: 437 vs 155 (`all`, q = 10 ms), 1,414 vs 89 (`cpu4`),
  8,064 vs 679 (`scale10`). Each extra switch costs about 0.13 ms of emulated
  time (`cpu4`: +155 ms makespan over ~1,140 extra switches), and the
  scheduler's own decision time is 1.2-2.4% of runtime (9% at `scale10`, where
  the O(n) scan per decision shows).
* *CPU-bound turnaround*: +14% (`all`), +7% (`cpu4`), +21% (`scale10`) vs `hint`
  (+4%, +6% and +18% vs round robin).
* The cost falls as the quantum grows, and the latency benefit doesn't: at
  q = 50 ms `fair` has 185 switches (vs 145 for `hint`), +2.6% makespan, and
  0.06 ms interactive latency. So the cost is a tuning choice, not a verdict.
* But I/O-bound latency does grow with the quantum under `fair` (3.4 ms at
  q = 10, 9 ms at q = 50) while `hint` stays at 0.35-0.39 ms. That's the one
  place a declared hint beats measurement in these data: an I/O-style process
  does ~4.8 ms of work between waits, and `fair` can't tell it from a CPU-bound
  process until it has consumed CPU.

**What this does to the research question.** "Can compiler-derived hints help
the scheduler?" has a more defensible answer than before, and a narrower one:
*hints help relative to textbook schedulers, and relative to a CFS-style one
only on cost (fewer switches, less scheduler time, ~7-21% better CPU-bound
turnaround) and on I/O-style latency. They don't help interactive latency,
where measurement alone is enough.* The interesting open question is the
combination: use the hint to skip fair's learning period and cut its switching
(e.g. larger slices for processes hinted `cpu`), while measurement catches wrong
and stale hints. That policy isn't built; MLFQ-with-hints (`hmlfq`) was the
wrong vehicle, because MLFQ's demotion erased the hint (F4).

**Caveats specific to this baseline.** `fair` is *my* CFS-style implementation
(no weights, no nice levels, one CPU, no group scheduling, no EEVDF lag and
deadlines), not Linux's scheduler. Its wake-up granularity (1 ms) and sleeper
credit (Q/2) are Linux-like defaults I scaled to this kernel's 1 ms tick, not
swept. Its numbers could move with those, and with a real CFS/EEVDF the
switch-count cost might look different.

## Results: compiler-derived hints (PlainLang workloads)

130 runs, 10 seeds per configuration; every hint below was produced by
`plainhint.py` from the program's source, not chosen by me.

**F5 — When the compiler gets it right, the built-in result reproduces with
real compiled programs.** `pl_mixed`: interactive p95 0.03 ms with `hint` vs
7.7 ms (`rr`) and 7.1 ms (`mlfq`). The I/O-style program: 0.07 ms vs 16 ms
(`rr`) and 3.0 ms (`mlfq`). Unlike `wio`, its bursts (~2.8 ms) are short enough
that MLFQ keeps it fairly high.

**F6 — A static hint can't follow a program that changes (the clearest failure
of the idea).** `p_phase` spends its first half waiting for events and its second
half computing. The compiler (correctly, by total work) labels it `cpu`, so
during its interactive half `hint` treats it as CPU-bound. Its p95 latency is
70.7 ms, against 5.0 ms for round robin and 6.1 ms for MLFQ, which notices the
behaviour change. `hmlfq` lands in between (30 ± 20 ms). This is the
strongest argument for the "runtime feedback" extension (S2 in the original
plan), and against static hints alone.

**F7 — A misclassified program shifts the cost onto its neighbours.**
`p_trick` does ~3 ms of work per wait, but its loop is a `while`, so the
classifier assumes 100 iterations and labels it `interactive`. Under `hint` the
mislabelled program itself does great (0.03 ms). The genuinely interactive
`p_int` pays: its p95 rises from 0.03 ms (in `pl_mixed`) to 2.88 ms, because it
now shares the top priority level with a process that runs 3 ms bursts. That's
still better than `rr` (7.9 ms) or `mlfq` (8.4 ms) in this mix. But the general
lesson is that a wrong "interactive" label is an unearned priority, and someone
else pays for it. A real system would need to verify or police hints (e.g.
demote a process whose behaviour contradicts its hint), which is exactly what
`hmlfq` tries and, in this design, overdoes.

**What this does *not* show.** That hints beat a well-tuned production
scheduler (no CFS/EEVDF-style baseline yet). That the effect survives real
hardware, real I/O devices, or real interactive users. That 30 µs vs 10 ms
matters to anyone. On this emulator every instruction costs the same, so the
absolute numbers are not hardware numbers.

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
  deadlock) made long batches crash or hang.
* "stalls" that turned out to be the **host laptop entering Modern Standby**
  (matched to the second against the Windows event log). Completed runs are
  unaffected, because `-icount` guest time counts instructions. Interrupted
  batches were rerun, and the harness now treats a host pause as a pause, not a hang. Each was found from the harness's
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
   proves little, and F8 shows exactly that happening: against a CFS-style
   baseline the interactive-latency advantage disappears. The `fair` policy
   closes the main gap a reviewer would raise, but it's still my approximation
   of CFS. Comparison against the real Linux scheduler (or at least a more
   faithful CFS/EEVDF port) remains open.

## Not done yet (the honest list)

- [x] **PlainLang → AetherOS backend** (`plainaxe.py`): hand-encoded x86-64,
  differential-tested on 232 programs against the interpreter inside the kernel.
- [x] **Compiler-derived hints** (`plainhint.py`), with a written rationale per
  program.
- [x] **SSE state save/restore**, verified with a negative control.
- [ ] **Hint-classifier accuracy on real programs**, not just five workloads I
  wrote. E.g. classify every PlainLang lesson/example and compare with measured
  behaviour.
- [x] **A fair-queuing baseline** (`fair`, CFS-style). It changed the main finding (F8-F10).
- [ ] **A policy that combines hints with fair queuing** (hint-sized slices, measurement
  to catch wrong or stale hints). F8-F10 say this is the interesting design.
- [ ] **Sweeps of `fair`'s own parameters** (wake-up granularity, sleeper credit).
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
