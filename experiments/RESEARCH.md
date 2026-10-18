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

**Outcomes** (details in Results below):

- **H1** holds against round robin and MLFQ (F1). It does *not* hold in practice
  against a CFS-style scheduler, which gets the same latency with no hints
  (F5). I only tested that after the first write-up of this study; the first
  version of this document said H1 was simply supported.
- **H2** is false as stated: MLFQ recovered almost none of the benefit (F2). A
  *different* runtime-inference design (fair queuing) recovered all of it (F5).
- **H3** holds (F3).
- **H4** holds for the hint-only policy (F4). `hmlfq` limits the damage only
  because MLFQ erases the hint altogether (F2).
- **H5 (added after seeing F5-F8; not a pre-registered hypothesis).** Letting a
  hint influence only slice length, never priority, keeps fair queuing's
  latency and recovers most of the hint scheduler's context-switch savings, and a
  wrong hint can't hurt latency. Supported (F9), but this was formulated *after*
  looking at the data that suggested it, so it needs confirmation on workloads I
  haven't tuned against.

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
| **hfair** (added 2026-10-18) | measurement picks who runs; the hint only sizes slices | `fair`, but a thread hinted `cpu` gets a slice HFAIR_MULT times longer (default 4). The hint never changes priority, so a wrong hint costs efficiency, not latency |
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

## Results

**Primary dataset: the optimized (release) kernel.** 1,155 runs in 147
configurations, 10 seeds for the main matrix and 5 for sweeps, zero plausibility
warnings. Tables with 95% CIs are in `report_release/report.md`, graphs next to it.
Everything below is from that dataset unless it says otherwise.

An earlier dataset (`results/`, `report/`, 1,035 runs) was measured on the
*unoptimized debug* kernel. It's kept, but it overstates every policy's
*cost*: scheduler decision time was 33-48 us per switch against 2.5-3.4 us when
optimized, and policy makespans differed by up to 6% against under 1%. My first
write-up of this study drew cost conclusions from the debug data. They were
wrong and are corrected below. The *latency* conclusions did not change between
builds, since they depend on scheduling decisions rather than on speed.

"Latency" is the time from a process being woken to it actually running (p95).
Base quantum 10 ms unless stated. Policies: `rr` round robin; `mlfq` textbook
multi-level feedback queue; `hint` priority from the declared hint; `hmlfq` hint
as MLFQ's starting level; `fair` a CFS-style baseline that uses no hints;
`hfair` fair queuing where hints only size time slices.

### Against textbook schedulers, hints look excellent

**F1 - Correct hints cut interactive latency by three to four orders of
magnitude relative to round robin and MLFQ.** Workload `all`: 2 us (`hint`)
against 9.8 ms (`rr`) and 8.0 ms (`mlfq`). A woken interactive process has
strictly higher priority, so it preempts within one timer tick. The advantage
shrinks as CPU-bound competitors are added: 203 us with six, and 3.6 ms (+/-3.7
across seeds) with ten, against 85 ms (`rr`) and 45 ms (`mlfq`). The cause is
the aging rule (F11): with aging off, `hint` is 4 us at ten competitors.

**F2 - MLFQ with the textbook periodic boost recovers almost none of that;
without the boost it recovers all of it for strictly interactive work, at a
price (revised - see F13).** With my default 200 ms boost, interactive latency
under `mlfq` is 8.0 ms against 9.8 ms for `rr` (`all`) and 15.7 ms against
15.0 ms (`interactive`), and the I/O-style process is slower than under round
robin (21 ms vs 17 ms in `all`, 29 ms vs 21 ms in `mixed`). `hmlfq` behaves like
`mlfq` whatever the hints say (`interactive`: 15.8 / 16.3 / 16.9 ms with
correct / no / wrong hints): MLFQ's demotion erases the hint. **The first version
of this document stated F2 without the qualification. The 200 ms boost was my
choice, and the conclusion depends on it** (F13).

**F3 - With nothing to distinguish, nothing changes.** On `cpu4` (four
identical CPU-bound processes) makespan is within 0.4% across every policy.

**F4 - Wrong hints are worse than none for the hint-only policy.** With
deliberately wrong hints `hint` gives the interactive process 106 ms p95
latency, against 30-45 ms with *no* hints and ~10-15 ms under round robin. It is
bounded only by the 100 ms aging rule (F12), and the run takes 12% longer
overall (1,618 ms vs 1,443 ms).

### Against a CFS-style baseline, most of the advantage disappears

**F5 - A hint-free fair-queuing scheduler matches the hint scheduler on
interactive latency.** `fair` 3 us, `hint` 2 us, `rr` 9.8 ms (`all`). With
many competitors `fair` is the *better* of the two *at the default aging
setting*: with six, `fair` 4 us vs `hint` 203 us; with ten, `fair` 5 us vs `hint`
3.6 ms. With aging off they're equal (F11). It holds from q = 5 to
50 ms (3 us), and at q = 2 ms it's 203 us because the 1 ms slice floor limits
reaction time. Mechanism: a sleeper has used little CPU, so its virtual runtime
is small, so it preempts. Measured instead of declared.

**F6 - It also handles the programs that broke the hint scheduler.** A
PlainLang program that is interactive for half its life and CPU-bound for the
rest (`pl_phase`): p95 latency during its interactive half is 65 ms under `hint`,
5 ms under `rr`, 0.002 ms under `fair`. A program the classifier mislabels
(`pl_trick`): the genuinely interactive neighbour sees 0.73 ms under `hint` (vs
0.002 ms when the hint is right) and 0.003 ms under `fair`.

**F7 - What hints still buy: fewer context switches, not throughput.** On the
optimized kernel, makespan is the same for every policy to within ~1%
(`scale10`: `fair` 6,510 ms vs `rr` 6,440 ms; `hint` 6,452 ms). Total work
done per second doesn't change. What changes is how often the CPU is switched:

| workload | rr | fair | hint |
|---|---|---|---|
| `cpu4` | 268 | 1,295 | 86 |
| `all` | 201 | 419 | 153 |
| `scale6` | 437 | 3,938 | 248 |
| `scale10` | 706 | 6,564 | 620 |

Each switch costs ~3 us of decision code plus the fixed switch work here. The
emulator has no caches or TLB, so on real hardware a switch costs more than this
suggests, and `fair`'s switch rate would hurt more. That is a reason to take
the switch-count gap seriously rather than the near-zero makespan gap.

Mean turnaround of CPU-bound processes is better under `hint` (`all`: 1,309 ms vs
1,444 ms for `fair`), but **that is ordering, not speed.** `hint` runs CPU-bound
processes in long slices, so they finish one after another instead of all
finishing together at the end (classic: run-to-completion lowers *mean*
turnaround at equal total work). An earlier version of this document called it a
throughput gain. It isn't one.

**F8 - Hints still win on I/O-bound latency at moderate quanta.** The
I/O-style process in `all`: `hint` 0.62 ms, `fair` 1.5 ms, `hfair` 2.0 ms; in
`mixed`: 1.8 ms vs 3.0 ms vs 1.0 ms. Smaller than the gaps in F1-F2, not
consistently in `hint`'s favour, and it grows with the quantum under `fair` (7.4
ms at q = 20). So it's the one place a declared hint beats measurement here.

### The baselines' own tuning constants change the answers

Two constants I chose when writing the baselines turned out to matter: `hint`'s
aging threshold (100 ms) and MLFQ's boost period (200 ms). The sweeps
(`plans/aging_boost.txt`, 145 runs) were written with predictions in the plan
file's header *before* running, and are scored here honestly.

**F11 - The hint scheduler's degradation with many competitors is caused by its
aging rule (prediction P1: confirmed).** Interactive p95 latency at ten
CPU-bound competitors, by aging threshold: 10 ms -> 4.8 ms; 50 ms -> 5.6 ms;
100 ms (default) -> 3.6 ms; 200 ms -> 1.6 ms; **off -> 4 us in all five seeds.**
The mechanism: each CPU-bound process waits several hundred ms between turns, so
all of them get aged up to the interactive level, tie with the interactive
process, and one of them runs a slice before it. Aging exists to prevent
starvation, and removing it removes a latency tail but, see F12, also the
starvation protection.

**F12 - With wrong hints, latency tracks the aging threshold, and without
aging the victim starves, which wake-up latency cannot see (prediction P2:
direction right, numbers and the metric wrong).** Interactive p95 latency under
wrong hints: 27 ms (aging 10), 67 ms (50), 106 ms (100), 206 ms (200). That's
about the threshold plus 6-17 ms, a little above the ranges I predicted. I
predicted starvation with aging off, but the p95 and even the *maximum* wake-up
latency were tiny (813 us and 1.8 ms). The starvation is real, and it shows
up in other columns: the victim's **response time (first dispatch) is 1,286 ms**
and its turnaround 1,751 ms (450 ms with correct hints). Wake-up latency only
counts samples after a thread's first sleep, so a thread that never gets to run
in the first place is invisible to it. Reports now include response time and
turnaround for the interactive class. The earlier tables (F1-F10) used wake-up
latency alone; where they imply "no starvation" they imply less than they
seem to.

**F13 - MLFQ's boost *hurts* interactive latency; without it MLFQ matches the
hint scheduler for interactive work and wrecks I/O-style work (prediction P3:
wrong).** I predicted shorter boost periods would improve interactive latency
monotonically. The opposite happened. In `all`, interactive p95 latency by boost
period: 25 ms -> 9.8 ms; 50 -> 10.4; 100 -> 14.2; 200 (default) -> 8.4;
**1000 ms or off -> 1 us**. At ten competitors: 85 ms with a boost every 25-100
ms, 45 ms at the default, **2 us** at 1000 ms or off. The boost periodically
returns every CPU-bound thread to the interactive process's level, which is
exactly what MLFQ's demotion was supposed to prevent. The price is paid by the
I/O-style process: with the boost off its p95 latency is **76 ms** (max 116 ms)
against 6-23 ms with a boost, 1.5 ms for `fair` and 0.6 ms for `hint`; it is
sunk to the bottom level and stays there. It also risks starving CPU-bound
work, though not at these workload sizes (makespan +2% with the boost off).

**What F11-F13 mean for F1-F10.** My first conclusion, "hints beat runtime
inference by orders of magnitude", compared hints to an MLFQ whose boost
period I had set to a value that cripples interactive latency. A reviewer who
tuned the baseline differently would have got a different headline. The
fair-queuing comparison (F5) doesn't have that problem, because `fair` has no
period constant that trades latency against anything. It's still true that
nothing in this study has been tuned for any policy except by the sweeps above.

**F14 - `fair`'s latency does not depend on its own tuning constants in these
workloads, for structural reasons I had to find by testing my prediction
(G1: wrong; G2: mostly wrong; G3: confirmed).** Sweeping wake-up granularity
(0-20 ms) and sleeper credit (0-100% of the quantum) over `all`, `interactive`
and `scale10` (180 runs, `plans/fair_knobs.txt`): interactive p95 latency stays at
3-6 us for every granularity, and for every credit except 0, which gives 1.0 ms
in `all` and nothing in the other two. Makespan differs by under 0.01%. My
prediction G1 said granularity above the 5 ms credit would push latency to
milliseconds. I had forgotten that my own `fair` has a *second* preemption rule
("preempt if a waiting thread has fallen a whole slice behind"), which fires
first for any sleeper whose lag exceeds the 1-2.5 ms slice. A null result from a
knob that does nothing could also mean the knob isn't wired up, so I tested that
(`plans/fair_lag.txt`, predictions again written first): with the slice-lag rule
switched off, granularity at or above the credit raises latency in `all` from 3 us
to 1.0 ms, as predicted, and credit 0 does the same. So the knobs are wired up
and the original null result is real. In `scale10` nothing moves even then:
with ten competitors the slice is 1 ms, so the scheduler reconsiders on every
tick and always picks the smallest virtual runtime, which makes the preemption
rules moot.

The consequence: `fair`'s low latency in this study comes from small slices plus
picking the least-served thread, not from a lucky choice of constants. It could
change with large slices and few competitors, where the preemption rules would
carry the weight, and I haven't tested that regime beyond the quantum sweep
(where it held, thanks to the slice-lag rule).

### A design that combines them: hints size slices, measurement picks who runs

**F9 - `hfair` keeps `fair`'s latency and recovers most of `hint`'s switch
count, and a wrong hint cannot hurt latency.** In `hfair`, a thread hinted `cpu`
gets a slice HFAIR_MULT times longer. That's the only thing a hint does. It never
changes who is picked (virtual runtime alone decides), so a wrong hint can only
cost some efficiency.

* *Switches*, by multiplier (default 4):

| workload | fair | x2 | x4 | x8 | x16 | hint |
|---|---|---|---|---|---|---|
| `cpu4` | 1,295 | 652 | 332 | 172 | 92 | 86 |
| `all` | 419 | 282 | 213 | 171 | 157 | 153 |
| `scale6` | 3,938 | 2,003 | 1,025 | 537 | 293 | 248 |

At x16, `hfair` is within 3-18% of `hint`'s switch count, and interactive
latency stays at 3-4 us. The multiplier is a knob: x4 is a modest choice, x16
trades response time to *CPU-bound* work (a CPU-hinted process now holds the CPU
for up to 16 slices) for fewer switches.
* *Wrong hints* (CPU work labelled interactive and vice versa): interactive latency
stays 3 us in `all` and `interactive` (`hint`: 106 ms). Switches rise to
`fair`'s level (`cpu4`: 1,296; `all`: 392), which is the correct degradation:
a wrong hint removes the optimisation but nothing else.
* *PlainLang programs with compiler-derived hints*: `hfair` interactive latency
0.003 ms in `pl_mixed`, `pl_counted`, `pl_opaque` and `pl_trick`, with 173-183
switches against `fair`'s 318-322 and `hint`'s 139-144.

### What the compiler side taught us

**F10 - Better static analysis fixes the misclassification, and a blind spot
remains.** `p_trick`'s `while` loop was assumed to run 100 times (it runs
10,000), so the classifier called it `interactive` instead of `io`. The new
counted-loop analysis (`plainhint.py`, constant propagation plus a conservative
loop-counter recogniser) gets the exact 10,000 and the correct hint. The old
analysis is kept (`--analysis basic`) so the earlier programs reproduce
bit-for-bit. `p_opaque` hides the same loop's bound behind a function call, which
the classifier never evaluates, and it is misclassified again. Cost to its
interactive neighbour under `hint`:

| program | classifier | hint | neighbour's p95 latency |
|---|---|---|---|
| p_trick_c (counted analysis) | right | io | 0.002 ms |
| p_trick (basic analysis) | wrong | interactive | 0.73 ms |
| p_opaque (counted analysis) | wrong | interactive | 0.65 ms |

Still far better than round robin's 7.7 ms, and exactly 0.003 ms under `hfair`
in all three cases.

### What this changes about the research question

The question was "can compiler-derived information help the scheduler?" The
answer these data support is narrower than the first one I wrote:

1. Against textbook schedulers, hints help a great deal (F1, F2).
2. Against a CFS-style scheduler, they don't help *latency* for interactive
   processes, because runtime measurement already gives it (F5, F6).
3. What a hint can still add is knowledge about the *future* that measurement
   lacks: "this process will want the CPU for a long time, so don't switch it
   out every millisecond" (F7, F9), and moderate I/O-bound latency gains (F8).
4. The robust way to use a hint is to let it influence only an efficiency
   parameter, never priority (F9). The hint-as-priority design has a failure
   mode (F4, F6, F10) that this one doesn't.

**What this does *not* show.** That any of this holds against the real Linux
scheduler (`fair` is my approximation of CFS: no weights, no nice levels, one
CPU, no group scheduling, no EEVDF deadlines, and its wake-up granularity and
sleeper credit are scaled defaults, not swept). That it holds on real hardware,
with real I/O devices or real interactive users: the emulator charges every
instruction the same, there are no caches, and a context switch costs far less
than on a real machine. That 3 us vs 10 ms matters to anyone. And the
workloads are synthetic.

## How to reproduce

```bash
# Everything, on the optimized kernel (the primary dataset), 6 QEMUs at a time.
# The PlainLang workloads (experiments/plainlang/*.axe) are compiled by the
# PlainLang repo's plainaxe.py and are checked in:
#   python ../plainlang/plainaxe.py experiments/plainlang/p_cpu.plang --analysis basic
cargo bootimage --release
experiments/run_all.sh target/x86_64-unknown-none/release/bootimage-aetheros_kernel.bin \
                       experiments/results/release 6
python experiments/analyze.py experiments/results/release/*/ --out experiments/report_release

# Or one plan on either build (drop --bin for the debug kernel):
python experiments/aetherbench.py --script experiments/plans/main_all.txt \
       --out experiments/results/main_all --files experiments/plainlang/*.axe
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
* **a benchmark line didn't mean the same thing everywhere.** `bench` inherited
  the previous run's quantum, so plan lines without `q=` silently ran at the
  quantum of the line above. The `hfair` multiplier sweep ran at q = 50 instead
  of 10 and was caught only because its numbers didn't match the main matrix.
  Defaults are now constants, and the sweep was rerun;
* **benchmarking an unoptimized kernel** inflated every scheduling cost 10x and
  led to wrong cost conclusions (see Results). The release build is the primary
  dataset now;
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
4. **Policy design choices matter, and F11-F13 show how much.** `hint`'s aging,
   MLFQ's boost and `fair`'s granularity and credit are swept now (F11-F14;
   `fair` turned out to be insensitive). Not swept: the 3 MLFQ levels, the
   hint-to-level mapping, `hint`'s quantum scaling, and `fair`'s slice
   floor. Any of them could move conclusions the way the boost period did. Results depend on Q = 10 ms, 3 levels,
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
- [x] **A policy that combines hints with fair queuing** (`hfair`; F9).
- [ ] **Sweeps of `fair`'s own parameters** (wake-up granularity, sleeper credit).
- [x] **Sweeps of boost period and aging threshold** (F11-F13).
- [x] **`fair`'s wake-up granularity and sleeper credit** (F14: insensitive).
- [ ] **Sweep the remaining constants** (MLFQ levels, hint-to-level mapping, `hint`'s quantum scaling, `fair`'s 1 ms slice floor).
- [ ] **Report starvation properly**: response time and max latency alongside p95 for every class, in the main tables, and re-read F1-F10 with that in mind.
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
