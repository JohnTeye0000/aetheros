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

**Primary dataset: the optimized (release) kernel.** 1,550 runs in 215
configurations, 10 seeds for the main matrix and 5 for sweeps (plus 1,440 runs
on 30 random workloads, F15), zero plausibility
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
against 9.8 ms (`rr`) and 6.8 ms (`mlfq`). A woken interactive process has
strictly higher priority, so it preempts within one timer tick. The advantage
shrinks as CPU-bound competitors are added: 203 us with six, and 3.6 ms (+/-3.7
across seeds) with ten, against 85 ms (`rr`) and 45 ms (`mlfq`). The cause is
the aging rule (F11): with aging off, `hint` is 4 us at ten competitors.

**F2 - MLFQ with the textbook periodic boost recovers almost none of that;
without the boost it recovers all of it for strictly interactive work, at a
price (revised - see F13).** With my default 200 ms boost, interactive latency
under `mlfq` is 6.8 ms against 9.8 ms for `rr` (`all`) and 15.0 ms against
15.0 ms (`interactive`), and the I/O-style process is slower than under round
robin (35 ms vs 17 ms in `all`, 30 ms vs 21 ms in `mixed`). `hmlfq` behaves like
`mlfq` whatever the hints say (`interactive`: 16.5 / 15.0 / 16.0 ms with
correct / no / wrong hints): MLFQ's demotion erases the hint. (Numbers from the
rerun after the boost-clock fix; before it, MLFQ-family values depended on
plan order. See "Measurement problems".) **The first version
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
wrong; qualified by F20: only after an ~88 ms start-up wait).** I predicted shorter boost periods would improve interactive latency
monotonically. The opposite happened. In `all`, interactive p95 latency by boost
period: 25, 50 and 100 ms -> 9.8 ms; 200 (default) -> 6.8;
**1000 ms or off -> 1 us**. At ten competitors: 85 ms with a boost every 25-100
ms, 45 ms at the default, **2 us** at 1000 ms or off. The boost periodically
returns every CPU-bound thread to the interactive process's level, which is
exactly what MLFQ's demotion was supposed to prevent. The price is paid by the
I/O-style process: with the boost off its p95 latency is **76 ms** (max 116 ms)
against 6-35 ms with a boost, 1.5 ms for `fair` and 0.6 ms for `hint`; it is
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

### Out of sample: 30 random workloads (F15)

Every workload above was picked by me, and `hfair` was designed after looking
at the data it was supposed to explain. So `gen_random_mix.py` draws 30
workloads (1-8 CPU-bound, 0-3 io-style and 0-3 interactive processes) from a
fixed seed and runs each under eight policy configurations, 3 seeds each: 720
runs per batch. The predictions sit in the plan headers and were written
before each batch ran. `mix_summary.py` scores them mechanically. Reports:
`report_release/random_mix.md` (v1), `report_release/random_mix2.md` (v2).

**v1 (`mix:`; every interactive process sleeps exactly 15 ms).**

| prediction | result |
|---|---|
| M1 fair and hfair interactive p95 <= 50 us in every mix | **failed** 5/21 |
| M2 hint <= hfair <= 1.05 x fair in context switches | held 30/30 |
| M3a hfair with wrong hints <= 50 us in every mix | **failed** 5/21 |
| M3b hint with wrong hints > 10 ms in a third of mixes | held 21/21 |
| M4 makespan within 1% | **failed** 23/30 (worst 1.14%) |
| M5 mlfq worst of the hint-free policies | **failed** 4/21 |

M1 and M3a failed because of my workload, not the schedulers: with two or more
interactive processes on identical 15 ms periods, they all woke in the same
tick and queued behind each other. No single-CPU scheduler can avoid that. M5
was ill-posed: rr and mlfq are both tens of ms behind fair, and which of the two
is worse is noise. Single-interactive mixes did behave as predicted (fair,
hfair and hint 3-6 us; rr and mlfq 39-75 ms).

**v2 (`mix2:`; interactive periods staggered 13/15/17 ms; same 30 mixes;
predictions rewritten after v1, before running).**

| prediction | result |
|---|---|
| M1' fair and hfair p95 <= 100 us in >= 90% of mixes | **failed** 13/21 |
| M2 hint <= hfair <= 1.05 x fair in switches | held 30/30 |
| M3a' hfair with wrong hints <= 100 us in >= 90% | **failed** 17/21 |
| M3b hint with wrong hints > 10 ms in a third | held 21/21 |
| M4' makespan within 1.5% | held 30/30 (worst 1.14%) |
| M5' rr's and mlfq's p95 worse than fair's | held 21/21 |
| M6a hint (correct hints) <= 100 us in >= 90% of mixes with <= 5 CPU-bound | **failed** 6/11 |
| M6b hint > 100 us in at least one mix with >= 7 CPU-bound | held (6/8) |

Staggering didn't remove the collisions: every M1' failure is a mix with
**three** interactive processes. The explanation, which I worked out *after*
seeing the failures (so treat it as a hypothesis, not a result), is the
1 ms timer tick. Wake-ups are quantized to ticks, and two processes whose
wake-ups land in the same tick queue behind each other whatever the periods
are. With per-wake collision probability about 1/15 + 1/17 and half of the
colliding wake-ups being second in line, about 6% of wake-ups for three
interactive processes should be delayed by one burst (~0.17-0.33 ms), and
about 3% for two. Measured under `fair`: **0.0% (one interactive), 3.3% (two), 5.2%
(three)** of wake-ups over 100 us. The p95 threshold sits right on that edge,
so p95 flips for I = 3 and not for I = 2. A metric threshold placed at p95
turned a 5% effect into a pass/fail cliff, which is a lesson about the metric
as much as the scheduler.

What the random workloads support:

* **The structural claims replicate out of sample.** The switch ordering
  hint <= hfair <= fair (M2) held in 60/60 mix-batches, wrong hints break
  hint-as-priority in every mix with both CPU-bound and interactive processes
  (M3b, 42/42, worst interactive response time 164 ms), makespan
  stays within ~1.1%, and fair beats rr and mlfq everywhere (M5').
* **hfair's wrong-hint robustness mostly replicates** (17/21; every failure is a
  three-interactive mix at 0.18-0.19 ms, against 107-150 ms for hint).
* **hint with *correct* hints is less robust than the hand-picked workloads
  suggested.** Its p95 reaches 0.33-2.7 ms in 15 of 21 mixes. Most of these mixes
  have two or three interactive processes that hint ranks equally, so some of the
  latency comes from them queuing behind each other and some from aging (F11).
  The two aren't separated yet.
* **"Interactive latency of a few microseconds" holds for one interactive
  process.** With several, every policy is bounded below by one interactive
  process's burst, and the comparison has to be made on p90 or on the fraction
  of delayed wake-ups, not on p95 against a fixed threshold.

### Testing the after-the-fact explanation (F16)

The tick-collision story above was invented after seeing the failures, so it
got its own run with predictions written first (`plans/collisions.txt`,
99 runs, scored by `collisions.py`, report `report_release/collisions.md`).
All four held:

* **C1, C2. The floor belongs to the workload, not the scheduler.** With *no*
  other processes at all, 2 interactive processes have 3.3% of wake-ups
  delayed and 3 have 5.6-5.8%, under every one of rr, mlfq, fair, hint and
  hfair (one interactive process: 0.0%). The delay is ~0.48 ms, one sibling's
  burst. p95 is 0-1 us for two and ~485 us for three, again under every
  policy.
* **C3. hint's extra latency in the random mixes with <= 2 interactive
  processes is all aging.** With aging off, p95 falls from 0.33-2.7 ms to
  3-4 us in all five such mixes.
* **C4. In three-interactive mixes it isn't aging.** With aging off, hint
  stays at 486-493 us: the C1 floor.

One correction to my own estimate: the delayed fractions are not random.
They are exactly 1 wake-up in 30 per colliding pair in every seed, because the
seed doesn't move the interactive processes' phases. The mechanism (same-tick
wake-ups) is what C1-C2 establish. The "probability" arithmetic just happened
to land near the right number.

**Consequence for every result above:** p95 interactive latency is a fair
summary only with one interactive process. With several, every policy
shares a floor set by tick quantization, and policies should be compared above
that floor (p90, or delayed fraction minus the C1 floor).

### How often is the compiler right? (F17)

Five hand-written programs say little about a classifier. `gen_hint_corpus.py`
draws 150 PlainLang programs from a fixed seed. They combine `repeat`/`count`
loops, counted `while` loops, `while` loops with an aliased counter, loop
bounds returned by a function, waits gated by an `if`, and two-phase programs.
Each one is compiled with `plainaxe.py`. Its static hint is recorded under
both analyses, and then it runs *alone* under `fair`, where its measured CPU
time per wait is the ground truth. Thresholds and predictions are in
`plans/hint_corpus.txt`. Scored by `hint_accuracy.py`, report
`report_release/hint_accuracy.md`.

| prediction | result |
|---|---|
| A1 counted analysis agrees with measurement on >= 70% | held: **76%** (balanced 76%) |
| A2 counted beats basic by >= 10 points | held: 76% vs **62%** |
| A3 gross errors (cpu <-> interactive) < 10%, mostly from blind-spot shapes | held: **2 of 150**, both from blind-spot shapes |
| A4 at least half the misses lie within 3x of the boundary crossed | **failed**: 17 of 36 |

Accuracy by shape: counted `while` 97%, `repeat` 91%, `count` 82%; the
documented blind spots are clearly worse: function-bound bounds 67%, gated
waits 62%, aliased counters 53%.

**The finding I didn't predict: all 36 misses go the same way.** Every wrong
guess is *lighter* than the truth (cpu guessed as io/interactive/none, io
guessed as interactive). Nothing measured lighter was ever guessed heavier. Two
rules in the classifier do this. A loop it can't count is assumed to run 100
times, and this corpus's loops usually run more. A gated wait is counted as if
it happened on every iteration, which overstates waits and understates work per
wait. The size of the bias depends on the corpus (trip counts here are
log-uniform from 5 to 60,000), but its direction follows from the rules.

**Why this matters for scheduling:** a lighter-than-true hint is exactly the
harmful kind under hint-as-priority. A CPU-heavy program labelled interactive
is the F4/F6 failure (interactive neighbours delayed by 10-100 ms). Under
`hfair` a too-light hint only shortens that program's own slices. So the
classifier's errors land precisely where the hint-as-priority design is
fragile. That makes F9's design argument stronger, and it suggests a cheap
fix to test: bias unknowns *heavier* (assume more trips, count gated waits as
rarer), trading accuracy for safety.

Caveats: the ground-truth thresholds are the classifier's own, converted at
its documented ~40 ns per operation, so near-boundary programs could flip with a
different conversion. Very short programs (e.g. 0.3 ms of CPU and one wait)
count as `io` by that rule even though they barely use the CPU. The corpus is
generated, not real user code.

### Biasing the classifier heavy doesn't fix it (F18)

F17 suggested a cheap fix: make the classifier guess *heavy* when unsure. A new
`safe` analysis in `plainhint.py` changes two rules. A loop of unknown length is
assumed to run 10,000 times instead of 100, and an `if` counts the *fewest*
waits of its branches. The predictions were committed to git *before* the
analysis was written (`plans/hint_safe_predictions.txt`, commit 6ddd1c7) and
scored on F17's already-measured programs, so no new kernel runs were needed.

| analysis | agreement | balanced | too light | too heavy |
|---|---|---|---|---|
| basic | 62% | 63% | 56 | 1 |
| counted | **76%** | 76% | 36 | 0 |
| safe | 59% | 62% | **11** | 50 |

S1 (too-light misses <= 9): **failed**, 11. S2 (agreement >= 61%): **failed**,
59%. S3 (too-heavy misses mostly from gated waits): **failed**, only 15 of 50.
Most came from the 10,000-trip assumption, which overshoots as badly as 100
undershoots. S4 (no measured-cpu program guessed interactive): held.

Some of `safe`'s 11 too-light misses are new and come from the rule itself. A
program whose only waits are gated gets 0 waits, and if its work is small it
becomes `none`, the lightest class. A rule that's safe in one dimension
interacted badly with another.

**What I take from it:** a single bias constant moves errors from one
direction to the other and doesn't remove them. The errors come from loop
bounds the analysis can't see, and no constant guess for an unknown bound is
right for programs whose bounds span four orders of magnitude. The two
directions do cost different people, though. Under hint-as-priority a too-light
guess hurts *other* processes (a hog gets priority), while a too-heavy guess
mostly hurts the *mislabelled program itself* (it loses priority). That
asymmetry follows from the design but wasn't measured here. So `safe` is a
defensible choice when protecting neighbours matters more than accuracy. Under
`hfair` neither direction can hurt neighbours, which is the F9 argument again.

### Who pays for a wrong hint? (F19)

F18 argued that a too-light hint hurts *neighbours* and a too-heavy one hurts
*the mislabelled program itself*. `gen_asymmetry.py` tested that in the kernel.
It took five corpus programs whose counted hint is too light (measured
CPU-heavy, guessed interactive) and five whose safe hint is too heavy
(measured interactive, guessed cpu). Each ran with each analysis's hint next
to `p_int` (a real interactive program) and `p_cpu` (a hog), under `hint` and
`hfair`, 3 seeds: 120 runs. The predictions were committed before running
(d3c2836). Report: `report_release/asymmetry.md`.

* **Y2 held, starkly. A too-heavy hint costs only the program itself.** All
  five programs' own p95 went from 1-177 us (correct hint) to **100 ms**
  (`hint` waits until aging rescues it, and the aging threshold is 100 ms),
  while the neighbour stayed at 1 us in every case.
* **Y1 failed. A too-light hint hurts neighbours less often than I predicted.**
  `p_int`'s p95 exceeded 1 ms in 2 of 5 cases (3.1 ms, 1.9 ms), 178 us in a
  third, and ~1-15 us in two. The harm scales with the mislabelled program's
  burst length. Short bursts at top priority barely delay anyone.
* **Y3 failed, for an instructive reason.** Under `hfair` the neighbour's p95
  was above 100 us in 4 of 20 cells. Those cells are two programs, *with both
  hints*: 537 us for both hints of h074, 1.2-1.9 ms for h126. A result that doesn't
  depend on the hint can't be the hint's fault. These programs wake often with
  ms-long bursts, so `p_int` sometimes wakes behind one: the F16 collision floor,
  which applies to any co-waking program and not only to interactive siblings.
  I should have predicted it from F16.

**Net:** under `hint`, the direction of a classifier's error decides who pays,
with a too-heavy error confined to the program that carries it. Under `hfair`,
neither direction moved the neighbour's latency in 8 of 10 programs, and the other
two differ by less than the collision floor.

### Starvation audit: re-reading F1-F14 with worst cases (F20)

p95 can't see a process that is starved once, and wake-up latency can't see
one that waits before its first dispatch (F12 found that the hard way).
`analyze.py` now reports the worst wake-up per class and lists every
configuration in which an interactive process waited more than 50 ms at least
once, before first dispatch or on any single wake-up (`report_release/report.md`,
"Starvation audit"). 31 of 215 configurations qualify.

* **The headline comparisons survive.** No configuration of `fair`, `hfair`, or
  `hint` with correct hints at the default quantum is on the list.
* **F13 needs a qualifier.** MLFQ with the boost off has p95 2 us at
  scale10, but its interactive process's *first* dispatch waited 88 ms, behind
  ten CPU-bound processes taking their top-level slices. Every MLFQ variant pays
  that start-up wait (85-90 ms), so the boost comparison stands, but "MLFQ
  without the boost matches fair" is true only after start-up.
* **hfair at q = 50:** p95 3 us, but one run's interactive process waited 52.6 ms
  for its first dispatch. That's a start-up cost of long base slices, not visible
  in any earlier table.
* The worst cases are the known ones: wrong hints under `hint` (106-220 ms, and
  1.29 s with aging off), long quanta, and many competitors under rr/mlfq.

### The last constants: MLFQ depth and fair's slice floor (F21)

Pre-registered in `plans/constants_sweep.txt` (115 runs, final image), scored
by `constants_sweep.py`, report `report_release/constants_sweep.md`.

* **K1 held:** one-level MLFQ is round robin (9,818 vs 9,817 us in `all`).
* **K4-K6 held:** fair's slice floor behaves exactly as designed. Switches in
  `scale10` fall to 0.50x at a 2 ms floor and 0.21x at 5 ms (predicted 0.50 and
  0.20). Interactive latency stays at 3-6 us at every floor, and makespan moves
  under 1%. So `fair`'s switch count, the main thing hints improved on (F7), is
  itself a tunable constant. A fair scheduler with a 5 ms floor needs no hints
  to cut its switches by 5x. That narrows F7: hints buy switch reductions
  *without* raising the floor, which matters only if a short floor is wanted for
  other reasons.
* **K2 failed:** switches in `cpu4` fall from 1 to 3 levels (268, 164, 140)
  and then stop (140 at 4 and 5). With four threads sharing a 200 ms boost
  period, none uses enough CPU between boosts to sink below level 3. The boost
  caps the depth that matters.
* **K3 failed:** depth does change interactive latency in `all`: 4.4 ms (2
  levels), 6.8 (3), 18.8 (4 and 5). Deeper levels give the two CPU-bound threads
  80-160 ms slices. My guess is that a woken interactive thread then waits out
  part of such a slice after each boost, but that mechanism hasn't been tested.
  The practical point is the same as F13: the MLFQ baseline's numbers hinge on
  constants I chose.

### The hint scheduler's own shape (F22)

The last two fixed choices in `hint` were made the same way the MLFQ boost
was: by me, untested. One is slice scaling (Q << level, so a CPU-hinted thread
gets 40 ms slices). The other is the mapping that puts io-hinted threads one
level below interactive ones. `plans/hint_shape.txt`, 55 runs, 5 seeds,
predictions committed first. Default lines rerun as a regression check matched
`main_*` within 0.1 ms.

| config | cpu4 switches | `all` interactive p95 | `all` io p95 | `mixed` io p95 | scale10 interactive p95 |
|---|---|---|---|---|---|
| hint (default) | 85 | 2 us | 0.69 ms | 1.90 ms | 3.60 ms |
| flat slices (`hscale=0`) | 267 | 2 us | 0.69 ms | 1.90 ms | **0.60 ms** |
| io on top level (`iotop=1`) | - | **818 us** | **0.002 ms** | 2.71 ms | - |

* **H1 held:** without slice scaling, `hint`'s switch count on `cpu4` is round
  robin's (267 vs 268). *All* of `hint`'s switch advantage in F7 comes from the
  long slices it gives CPU-hinted threads, not from priority.
* **H2 held:** flat slices leave interactive latency at 2 us and makespan
  within 0.1%.
* **H3 failed:** moving io to the top level made the io process in `all`
  near-instant (0.69 ms to 2 us), but in `mixed` (two io processes, no
  interactive) it got *worse* (1.9 to 2.7 ms). I don't have a tested explanation.
* **H4 held:** the interactive process now shares its level with ~4.8 ms io
  bursts, and its p95 rose from 2 us to 818 us (still well under rr's 9.8 ms).
  The mapping is a straight trade between the two classes.
* **Unpredicted:** flat slices cut `hint`'s worst case at ten competitors from
  3.6 ms to 0.60 ms. That fits F11's mechanism: aged CPU threads tie with the
  interactive process at the top level, and with flat slices the one that runs
  first holds the CPU for 10 ms instead of 40.

Together with F21, this sharpens F7. Every scheduler's switch count here is a
direct function of its slice length (MLFQ depth, fair's floor, hint's scaling).
What a hint contributes is *which* threads get long slices, which is exactly
the part `hfair` keeps.

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
# main report: the 18 hand-written plans (the random-mix, collision, corpus and
# asymmetry runs have their own scorers: mix_summary.py, collisions.py,
# hint_accuracy.py, asymmetry.py)
python experiments/analyze.py $(for p in main_cpu4 main_mixed main_interactive main_all scale quantum     plainlang fair_main fair_sweeps fair_plainlang hfair_main hfair_sweeps hfair_mult hfair_plainlang     plainlang_v2 aging_boost fair_knobs fair_lag; do echo experiments/results/release/$p; done)     --out experiments/report_release

# The classifier-accuracy corpus (F17) needs its own program files:
python experiments/gen_hint_corpus.py          # needs ../plainlang
python experiments/aetherbench.py --script experiments/plans/hint_corpus.txt        --out experiments/results/release/hint_corpus        --bin target/x86_64-unknown-none/release/bootimage-aetheros_kernel.bin        --files experiments/plainlang/corpus/*.axe
python experiments/hint_accuracy.py experiments/results/release/hint_corpus        --out experiments/report_release/hint_accuracy.md

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
* **MLFQ's boost clock carried over between runs.** Nothing reset it when a
  run started, so the boost phase depended on when the previous run ended. Identical
  mlfq/hmlfq runs (same seed) disagreed by up to 29 ms in io-style p95,
  depending on the order of lines in the plan. Found by a seed-for-seed regression
  check (`compare_runs.py`) after adding new knobs. It's fixed (`reset_counters`),
  the order effect is now at most one tick, and **every plan was rerun on the
  fixed image** (devlog 045);
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
   floor (both now swept, F21). Any of them could move conclusions the way the boost period did. Results depend on Q = 10 ms, 3 levels,
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
- [x] **Hint-classifier accuracy** on 150 generated programs against measured
  behaviour (F17). Still open: real user programs, not generated ones.
- [x] **A safety-biased classifier** (F18): trades errors from one direction to
  the other, 76% -> 59%. Not a fix.
- [x] **Measure the light/heavy cost asymmetry** in the kernel (F19).
- [x] **A fair-queuing baseline** (`fair`, CFS-style). It changed the main finding (F8-F10).
- [x] **A policy that combines hints with fair queuing** (`hfair`; F9).
- [x] **Sweeps of boost period and aging threshold** (F11-F13).
- [x] **`fair`'s wake-up granularity and sleeper credit** (F14: insensitive).
- [x] **Random, unpicked workloads** with pre-registered predictions (F15).
- [x] **Separate tie-queuing from aging** in hint's random-mix latency (F16: it's aging, plus the tick floor).
- [x] **MLFQ levels and fair's slice floor** (F21).
- [x] **Hint-to-level mapping and `hint`'s slice scaling** (F22).
- [x] **Report starvation properly** (F20): worst wake-up and a starvation audit in the main report; F13 qualified.
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
